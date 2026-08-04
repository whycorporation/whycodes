//! Image attachments for the prompt (drag-drop / paste of file paths).
//!
//! Terminals convert file drag-and-drop into a paste of one or more paths.
//! We detect image files, load + base64-encode them, and attach them to the
//! next user turn as multimodal `ContentBlock::Image` payloads.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use whycode_core::types::{ContentBlock, ImageSource};

/// Soft cap so a huge screenshot does not blow the request body.
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
/// Max images attached to a single prompt turn.
pub const MAX_ATTACHMENTS: usize = 10;

/// One image staged on the prompt, ready to send with the next message.
#[derive(Debug, Clone)]
pub struct PromptImage {
    pub path: PathBuf,
    pub label: String,
    pub media_type: String,
}

impl PromptImage {
    /// Display name (file name, or path fallback).
    pub fn label_for(path: &Path) -> String {
        path.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.display().to_string())
    }
}

/// Result of classifying pasted / dropped text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasteClassification {
    /// Paths that resolve to existing image files.
    pub images: Vec<PathBuf>,
    /// Remaining text to insert into the prompt (may be empty).
    pub text: String,
}

/// True when `path` looks like a supported raster image by extension.
pub fn is_image_extension(path: &Path) -> bool {
    media_type_for_path(path).is_some()
}

/// MIME type for common image extensions.
pub fn media_type_for_path(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "ico" => Some("image/x-icon"),
        "svg" => Some("image/svg+xml"),
        "heic" | "heif" => Some("image/heic"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

/// Normalize a pasted token into a filesystem path (quotes, `file://`, `~`).
pub fn normalize_path_token(token: &str) -> Option<PathBuf> {
    let mut s = token.trim();
    if s.is_empty() {
        return None;
    }
    // Strip matching quotes.
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        s = &s[1..s.len() - 1];
    }
    s = s.trim();
    if s.is_empty() {
        return None;
    }

    // file:// URI (optionally with host empty: file:///abs)
    if let Some(rest) = s.strip_prefix("file://") {
        let path_part = if rest.starts_with('/') {
            rest
        } else if let Some(idx) = rest.find('/') {
            // file://localhost/path
            &rest[idx..]
        } else {
            rest
        };
        // Percent-decode common spaces etc.
        let decoded = percent_decode(path_part);
        return Some(PathBuf::from(decoded));
    }

    if (s.starts_with("~/") || s == "~")
        && let Some(home) = home_dir()
    {
        if s == "~" {
            return Some(home);
        }
        return Some(home.join(&s[2..]));
    }

    Some(PathBuf::from(s))
}

/// If `token` points at an existing image file, return its absolute path.
pub fn resolve_image_path(token: &str) -> Option<PathBuf> {
    let path = normalize_path_token(token)?;
    if !is_image_extension(&path) {
        return None;
    }
    if !path.is_file() {
        return None;
    }
    // Prefer absolute for stable labels / reloads.
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// Split paste data into image paths and leftover text.
///
/// Tokens that are existing image files are attached; everything else is
/// re-joined as prompt text (whitespace preserved approximately).
pub fn classify_paste(data: &str) -> PasteClassification {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return PasteClassification {
            images: Vec::new(),
            text: String::new(),
        };
    }

    // Fast path: whole paste is one image path (common drag-drop).
    if let Some(path) = resolve_image_path(trimmed) {
        return PasteClassification {
            images: vec![path],
            text: String::new(),
        };
    }

    let tokens = tokenize(trimmed);
    if tokens.is_empty() {
        return PasteClassification {
            images: Vec::new(),
            text: data.to_string(),
        };
    }

    let mut images = Vec::new();
    let mut text_tokens = Vec::new();
    for tok in tokens {
        if let Some(path) = resolve_image_path(&tok) {
            if !images.iter().any(|p: &PathBuf| p == &path) {
                images.push(path);
            }
        } else {
            text_tokens.push(tok);
        }
    }

    // If nothing was an image, keep the original paste intact (newlines etc.).
    if images.is_empty() {
        return PasteClassification {
            images: Vec::new(),
            text: data.to_string(),
        };
    }

    PasteClassification {
        images,
        text: text_tokens.join(" "),
    }
}

/// Load an image file into a prompt attachment (metadata only until send).
pub fn load_prompt_image(path: &Path) -> Result<PromptImage, String> {
    let media_type = media_type_for_path(path)
        .ok_or_else(|| format!("not a supported image type: {}", path.display()))?
        .to_string();

    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }

    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} is too large ({:.1} MB; max {} MB)",
            PromptImage::label_for(path),
            meta.len() as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    if meta.len() == 0 {
        return Err(format!("{} is empty", PromptImage::label_for(path)));
    }

    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(PromptImage {
        label: PromptImage::label_for(&abs),
        path: abs,
        media_type,
    })
}

/// Read + base64-encode for the LLM request body.
pub fn encode_image_block(img: &PromptImage) -> Result<ContentBlock, String> {
    let bytes =
        std::fs::read(&img.path).map_err(|e| format!("read {}: {e}", img.path.display()))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} is too large ({:.1} MB; max {} MB)",
            img.label,
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    let data = STANDARD.encode(&bytes);
    Ok(ContentBlock::Image {
        source: ImageSource::Base64 {
            media_type: img.media_type.clone(),
            data,
        },
    })
}

/// Build user message content blocks from text + images.
pub fn build_user_blocks(text: &str, images: &[PromptImage]) -> Result<Vec<ContentBlock>, String> {
    let mut blocks = Vec::new();
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        blocks.push(ContentBlock::Text {
            text: trimmed.to_string(),
        });
    }
    for img in images {
        blocks.push(encode_image_block(img)?);
    }
    if blocks.is_empty() {
        return Err("empty message".into());
    }
    // Models usually want text present; if only images, add a short cue.
    if !images.is_empty() && trimmed.is_empty() {
        blocks.insert(
            0,
            ContentBlock::Text {
                text: "(image attached)".into(),
            },
        );
    }
    Ok(blocks)
}

// ── helpers ────────────────────────────────────────────────────────────

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Shell-ish tokens: whitespace-separated, optional single/double quotes.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    for c in chars {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn media_types() {
        assert_eq!(media_type_for_path(Path::new("a.PNG")), Some("image/png"));
        assert_eq!(media_type_for_path(Path::new("x.jpeg")), Some("image/jpeg"));
        assert_eq!(media_type_for_path(Path::new("a.txt")), None);
    }

    #[test]
    fn normalize_strips_quotes_and_file_uri() {
        let p = normalize_path_token("\"/tmp/shot.png\"").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/shot.png"));
        let p = normalize_path_token("file:///tmp/shot.png").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/shot.png"));
        let p = normalize_path_token("file:///tmp/my%20shot.png").unwrap();
        assert_eq!(p, PathBuf::from("/tmp/my shot.png"));
    }

    #[test]
    fn classify_whole_paste_as_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        std::fs::write(&path, b"\x89PNG\r\n").unwrap();
        let path_str = path.to_string_lossy().to_string();

        let c = classify_paste(&path_str);
        assert_eq!(c.images.len(), 1);
        assert!(c.text.is_empty());

        let quoted = format!("'{path_str}'");
        let c = classify_paste(&quoted);
        assert_eq!(c.images.len(), 1);

        let with_nl = format!("{path_str}\n");
        let c = classify_paste(&with_nl);
        assert_eq!(c.images.len(), 1);
    }

    #[test]
    fn classify_mixed_keeps_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        std::fs::write(&path, b"jpeg").unwrap();
        let paste = format!("what is this? {}", path.display());
        let c = classify_paste(&paste);
        assert_eq!(c.images.len(), 1);
        assert!(c.text.contains("what is this?"));
        // Image path token is stripped from leftover text.
        assert!(!c.text.contains("a.jpg"));
    }

    #[test]
    fn classify_plain_text_unchanged() {
        let c = classify_paste("hello world\nsecond line");
        assert!(c.images.is_empty());
        assert_eq!(c.text, "hello world\nsecond line");
    }

    #[test]
    fn load_and_encode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"fakepng").unwrap();
        let img = load_prompt_image(&path).unwrap();
        assert_eq!(img.media_type, "image/png");
        assert_eq!(img.label, "x.png");
        let block = encode_image_block(&img).unwrap();
        match block {
            ContentBlock::Image {
                source: ImageSource::Base64 { media_type, data },
            } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(STANDARD.decode(data).unwrap(), b"fakepng");
            }
            _ => panic!("expected image block"),
        }
    }

    #[test]
    fn build_blocks_image_only_adds_cue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("y.webp");
        std::fs::write(&path, b"webp").unwrap();
        let img = load_prompt_image(&path).unwrap();
        let blocks = build_user_blocks("", &[img]).unwrap();
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text.contains("image")));
        assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
    }

    #[test]
    fn tokenize_respects_quotes() {
        let t = tokenize(r#"/tmp/a.png "/tmp/my photo.png" rest"#);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1], "\"/tmp/my photo.png\"");
    }
}
