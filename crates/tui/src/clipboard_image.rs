//! Read a bitmap (or image file list) from the OS clipboard.
//!
//! Terminals never deliver PNG bytes as `Event::Paste` — that event is text
//! (paths, `file://`, or the screenshot tool's fallback string). Ctrl+V in
//! raw mode therefore has to ask the compositor / pasteboard itself.
//!
//! Tools match the text-copy side (`wl-copy` / `xclip` / `pbcopy`): we spawn
//! `wl-paste` / `xclip` / `pngpaste` / `osascript` / PowerShell. No extra crate.
//! A thread-local stub lets unit tests skip the live clipboard.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::images::{MAX_IMAGE_BYTES, resolve_image_path};

const TIMEOUT: Duration = Duration::from_millis(1500);

static STASH_SEQ: AtomicU32 = AtomicU32::new(1);

/// What Ctrl+V should do to the prompt.
///
/// Text is **not** read here. Hosts that intercept Ctrl+V already deliver
/// `Event::Paste`; reading `pbpaste`/`wl-paste` as well would double-insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptClipboard {
    Empty,
    /// Existing files (uri-list) or bytes we stashed under `clipboard-images/`.
    ImagePaths(Vec<PathBuf>),
    /// Treat Ctrl+V as a text paste (same as `Event::Paste`). Production
    /// `read_for_prompt` never returns this — only the test stub does.
    #[cfg(test)]
    Text(String),
}

#[derive(Debug)]
enum RunErr {
    NotFound,
    Timeout,
    TooLarge,
    Io(String),
    Exit,
}

/// Magic-byte hit used to pick a suffix / MIME before `load_prompt_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSniff {
    pub media_type: &'static str,
    pub ext: &'static str,
}

/// Identify a raster image from its header. `None` if the bytes are not a
/// format we already accept as a prompt attachment.
pub fn sniff_image(bytes: &[u8]) -> Option<ImageSniff> {
    if bytes.len() >= 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageSniff {
            media_type: "image/png",
            ext: "png",
        });
    }
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some(ImageSniff {
            media_type: "image/jpeg",
            ext: "jpg",
        });
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some(ImageSniff {
            media_type: "image/gif",
            ext: "gif",
        });
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageSniff {
            media_type: "image/webp",
            ext: "webp",
        });
    }
    if bytes.len() >= 2 && bytes.starts_with(b"BM") {
        return Some(ImageSniff {
            media_type: "image/bmp",
            ext: "bmp",
        });
    }
    if bytes.len() >= 4 && (bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*")) {
        return Some(ImageSniff {
            media_type: "image/tiff",
            ext: "tiff",
        });
    }
    if bytes.len() >= 4 && bytes[0] == 0 && bytes[1] == 0 && bytes[2] == 1 && bytes[3] == 0 {
        return Some(ImageSniff {
            media_type: "image/x-icon",
            ext: "ico",
        });
    }
    None
}

/// Write clipboard bytes into `dir` with a sniffed extension.
pub(crate) fn stash_image_bytes_in(bytes: &[u8], dir: &Path) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("clipboard image is empty".into());
    }
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "clipboard image is too large ({:.1} MB; max {} MB)",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_IMAGE_BYTES / (1024 * 1024)
        ));
    }
    let sniff = sniff_image(bytes).ok_or_else(|| {
        "clipboard data is not a recognized image (png/jpeg/gif/webp/bmp/tiff/ico)".to_string()
    })?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create clipboard-images dir: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::debug!(%error, "clipboard-images chmod 0700");
        }
    }
    prune_old_clipboard_images(dir);
    let seq = STASH_SEQ.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!(
        "paste-{}-{millis}-{seq}.{}",
        std::process::id(),
        sniff.ext
    ));
    std::fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

fn clipboard_images_dir() -> PathBuf {
    whycodes_core::paths::data_dir().join("clipboard-images")
}

fn prune_old_clipboard_images(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(24 * 60 * 60);
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < cutoff
            && let Err(error) = std::fs::remove_file(&path)
        {
            tracing::debug!(
                %error,
                path = %path.display(),
                "prune old clipboard image"
            );
        }
    }
}

fn stash_clipboard_image(bytes: &[u8]) -> Result<PathBuf, String> {
    stash_image_bytes_in(bytes, &clipboard_images_dir())
}

/// Ctrl+V entry: bitmap / uri-list only. Empty clipboard is a silent no-op.
pub fn read_for_prompt() -> Result<PromptClipboard, String> {
    #[cfg(test)]
    if let Some(stub) = stub_take() {
        return stub;
    }
    read_os_image()
}

fn read_os_image() -> Result<PromptClipboard, String> {
    #[cfg(target_os = "macos")]
    {
        return read_macos_image();
    }
    #[cfg(target_os = "windows")]
    {
        return read_windows_image();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        read_linux_image()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_linux_image() -> Result<PromptClipboard, String> {
    match read_wayland_image()? {
        PromptClipboard::Empty => {}
        other => return Ok(other),
    }
    match read_xclip_image()? {
        PromptClipboard::Empty => {}
        other => return Ok(other),
    }
    // No image (or no wl-paste/xclip). Silent — text Ctrl+V is the terminal's job.
    Ok(PromptClipboard::Empty)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_wayland_image() -> Result<PromptClipboard, String> {
    let types = match command_stdout("wl-paste", &["--list-types"], TIMEOUT) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(RunErr::NotFound | RunErr::Exit | RunErr::Timeout) => {
            return Ok(PromptClipboard::Empty);
        }
        Err(RunErr::TooLarge) => return Err("clipboard image is too large".into()),
        Err(RunErr::Io(e)) => return Err(e),
    };
    if let Some(mime) = first_image_mime(types.lines()) {
        return bytes_to_prompt(command_stdout("wl-paste", &["--type", mime], TIMEOUT));
    }
    if types
        .lines()
        .any(|l| l.eq_ignore_ascii_case("text/uri-list"))
    {
        match command_stdout("wl-paste", &["--type", "text/uri-list"], TIMEOUT) {
            Ok(bytes) => {
                let list = String::from_utf8_lossy(&bytes);
                let paths = parse_uri_list(&list);
                if !paths.is_empty() {
                    return Ok(PromptClipboard::ImagePaths(paths));
                }
            }
            Err(RunErr::NotFound | RunErr::Exit | RunErr::Timeout) => {}
            Err(RunErr::TooLarge) => return Err("clipboard image is too large".into()),
            Err(RunErr::Io(e)) => return Err(e),
        }
    }
    Ok(PromptClipboard::Empty)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_xclip_image() -> Result<PromptClipboard, String> {
    let types = match command_stdout(
        "xclip",
        &["-selection", "clipboard", "-t", "TARGETS", "-o"],
        TIMEOUT,
    ) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(RunErr::NotFound | RunErr::Exit | RunErr::Timeout) => {
            return Ok(PromptClipboard::Empty);
        }
        Err(RunErr::TooLarge) => return Err("clipboard image is too large".into()),
        Err(RunErr::Io(e)) => return Err(e),
    };
    let atoms: Vec<&str> = types.split_whitespace().collect();
    if let Some(mime) = first_image_mime(atoms.iter().copied()) {
        return bytes_to_prompt(command_stdout(
            "xclip",
            &["-selection", "clipboard", "-t", mime, "-o"],
            TIMEOUT,
        ));
    }
    if atoms
        .iter()
        .any(|a| a.eq_ignore_ascii_case("text/uri-list"))
    {
        match command_stdout(
            "xclip",
            &["-selection", "clipboard", "-t", "text/uri-list", "-o"],
            TIMEOUT,
        ) {
            Ok(bytes) => {
                let list = String::from_utf8_lossy(&bytes);
                let paths = parse_uri_list(&list);
                if !paths.is_empty() {
                    return Ok(PromptClipboard::ImagePaths(paths));
                }
            }
            Err(RunErr::NotFound | RunErr::Exit | RunErr::Timeout) => {}
            Err(RunErr::TooLarge) => return Err("clipboard image is too large".into()),
            Err(RunErr::Io(e)) => return Err(e),
        }
    }
    Ok(PromptClipboard::Empty)
}

#[cfg(target_os = "macos")]
fn read_macos_image() -> Result<PromptClipboard, String> {
    match command_stdout("pngpaste", &["-"], TIMEOUT) {
        Ok(bytes) => return bytes_to_prompt(Ok(bytes)),
        Err(RunErr::NotFound) => {}
        Err(RunErr::Exit) | Err(RunErr::Timeout) => {}
        Err(RunErr::TooLarge) => return Err("clipboard image is too large".into()),
        Err(RunErr::Io(e)) => return Err(e),
    }

    const JXA: &str = r#"
ObjC.import('AppKit');
function run() {
  var pb = $.NSPasteboard.generalPasteboard;
  var types = ['public.png', 'public.jpeg', 'public.tiff', 'public.gif'];
  for (var i = 0; i < types.length; i++) {
    var d = pb.dataForType(types[i]);
    if (d && d.length > 0) {
      return d.base64EncodedStringWithOptions(0).js;
    }
  }
  return '';
}
"#;
    match command_stdout("osascript", &["-l", "JavaScript", "-e", JXA], TIMEOUT) {
        Ok(bytes) => {
            let b64 = String::from_utf8_lossy(&bytes);
            let b64 = b64.trim();
            if b64.is_empty() {
                return Ok(PromptClipboard::Empty);
            }
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.as_bytes())
                    .map_err(|e| format!("decode macOS pasteboard: {e}"))?;
            bytes_to_prompt(Ok(decoded))
        }
        Err(RunErr::NotFound) => {
            Err("osascript is required to paste images (or install pngpaste)".into())
        }
        Err(RunErr::Exit) | Err(RunErr::Timeout) => Ok(PromptClipboard::Empty),
        Err(RunErr::TooLarge) => Err("clipboard image is too large".into()),
        Err(RunErr::Io(e)) => Err(e),
    }
}

#[cfg(target_os = "windows")]
fn read_windows_image() -> Result<PromptClipboard, String> {
    let dest = std::env::temp_dir().join(format!(
        "whycodes-clip-{}-{}.png",
        std::process::id(),
        STASH_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let dest_str = dest.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $img = [System.Windows.Forms.Clipboard]::GetImage(); \
         if ($null -eq $img) {{ exit 2 }}; \
         $img.Save('{dest_str}', [System.Drawing.Imaging.ImageFormat]::Png)"
    );
    let result = command_status(
        "powershell",
        &["-NoProfile", "-STA", "-Command", &script],
        TIMEOUT,
    );
    match result {
        Ok(()) => {
            let bytes = match std::fs::read(&dest) {
                Ok(b) => b,
                Err(e) => {
                    cleanup_temp(&dest);
                    return Err(format!("read clipboard temp: {e}"));
                }
            };
            cleanup_temp(&dest);
            bytes_to_prompt(Ok(bytes))
        }
        Err(RunErr::Exit) | Err(RunErr::Timeout) => {
            cleanup_temp(&dest);
            Ok(PromptClipboard::Empty)
        }
        Err(RunErr::NotFound) => {
            cleanup_temp(&dest);
            Err("PowerShell is required to paste images from the clipboard".into())
        }
        Err(RunErr::TooLarge) => {
            cleanup_temp(&dest);
            Err("clipboard image is too large".into())
        }
        Err(RunErr::Io(e)) => {
            cleanup_temp(&dest);
            Err(e)
        }
    }
}

#[cfg(target_os = "windows")]
fn cleanup_temp(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && path.exists()
    {
        tracing::debug!(%error, path = %path.display(), "clipboard paste: remove temp");
    }
}

fn first_image_mime<'a, I>(types: I) -> Option<&'static str>
where
    I: IntoIterator<Item = &'a str>,
{
    const PREFERRED: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/webp",
        "image/gif",
        "image/bmp",
        "image/tiff",
        "image/x-icon",
        "image/vnd.microsoft.icon",
    ];
    let lower: Vec<String> = types
        .into_iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    for pref in PREFERRED {
        if lower.iter().any(|t| t == pref) {
            return Some(*pref);
        }
    }
    None
}

/// `text/uri-list`: comments (`#`) skipped; `file://` and raw paths kept when
/// they resolve to an existing image.
pub(crate) fn parse_uri_list(data: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(path) = resolve_image_path(line)
            && !out.contains(&path)
        {
            out.push(path);
        }
    }
    out
}

fn bytes_to_prompt(result: Result<Vec<u8>, RunErr>) -> Result<PromptClipboard, String> {
    match result {
        Ok(bytes) => {
            if bytes.is_empty() {
                return Ok(PromptClipboard::Empty);
            }
            if sniff_image(&bytes).is_none() {
                return Ok(PromptClipboard::Empty);
            }
            let path = stash_clipboard_image(&bytes)?;
            Ok(PromptClipboard::ImagePaths(vec![path]))
        }
        Err(RunErr::NotFound | RunErr::Exit | RunErr::Timeout) => Ok(PromptClipboard::Empty),
        Err(RunErr::TooLarge) => Err(format!(
            "clipboard image is too large (max {} MB)",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )),
        Err(RunErr::Io(e)) => Err(e),
    }
}

fn send_run(tx: mpsc::Sender<Result<Vec<u8>, RunErr>>, value: Result<Vec<u8>, RunErr>) {
    if let Err(error) = tx.send(value) {
        tracing::debug!(?error, "clipboard paste: result receiver dropped");
    }
}

fn command_stdout(bin: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, RunErr> {
    let bin_owned = bin.to_string();
    let args_owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let output = Command::new(&bin_owned)
            .args(&args_owned)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(out) => {
                if (out.stdout.len() as u64) > MAX_IMAGE_BYTES {
                    send_run(tx, Err(RunErr::TooLarge));
                    return;
                }
                if out.status.success() {
                    send_run(tx, Ok(out.stdout));
                } else {
                    send_run(tx, Err(RunErr::Exit));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => send_run(tx, Err(RunErr::NotFound)),
            Err(e) => send_run(tx, Err(RunErr::Io(e.to_string()))),
        }
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_timeout) => Err(RunErr::Timeout),
    }
}

#[cfg(target_os = "windows")]
fn command_status(bin: &str, args: &[&str], timeout: Duration) -> Result<(), RunErr> {
    match command_stdout(bin, args, timeout) {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
thread_local! {
    static STUB: std::cell::RefCell<Option<Result<PromptClipboard, String>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn stub_take() -> Option<Result<PromptClipboard, String>> {
    STUB.with(|s| s.borrow().clone())
}

/// Install a clipboard result for the current thread (unit tests).
#[cfg(test)]
pub(crate) fn with_stub<R>(value: Result<PromptClipboard, String>, f: impl FnOnce() -> R) -> R {
    STUB.with(|s| *s.borrow_mut() = Some(value));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            STUB.with(|s| *s.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_png_jpeg_gif_webp_bmp_tiff_ico() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nxxxx").unwrap().ext, "png");
        assert_eq!(sniff_image(b"\xff\xd8\xff\xe0rest").unwrap().ext, "jpg");
        assert_eq!(sniff_image(b"GIF89a....").unwrap().ext, "gif");
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(sniff_image(&webp).unwrap().media_type, "image/webp");
        assert_eq!(sniff_image(b"BM6\0\0\0").unwrap().ext, "bmp");
        assert_eq!(sniff_image(b"II*\0....").unwrap().ext, "tiff");
        assert_eq!(sniff_image(&[0, 0, 1, 0, 1, 0]).unwrap().ext, "ico");
        assert!(sniff_image(b"hello").is_none());
        assert!(sniff_image(b"").is_none());
    }

    #[test]
    fn stash_writes_sniffed_extension() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"\x89PNG\r\n\x1a\nhello-png";
        let path = stash_image_bytes_in(bytes, dir.path()).unwrap();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn stash_rejects_unknown_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stash_image_bytes_in(b"", dir.path()).is_err());
        assert!(stash_image_bytes_in(b"not-an-image", dir.path()).is_err());
    }

    #[test]
    fn stash_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize((MAX_IMAGE_BYTES as usize) + 1, 0);
        let err = stash_image_bytes_in(&bytes, dir.path()).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn parse_uri_list_keeps_existing_images() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("shot.png");
        std::fs::write(&img, b"\x89PNG").unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, b"hi").unwrap();
        let list = format!(
            "# comment\nfile://{}\n{}\nhttp://example.test/x.png\n",
            img.display(),
            txt.display()
        );
        let paths = parse_uri_list(&list);
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert_eq!(
            paths[0].file_name().and_then(|n| n.to_str()),
            Some("shot.png")
        );
    }

    #[test]
    fn first_image_mime_prefers_png() {
        let types = "text/plain\nimage/jpeg\nimage/png\n";
        assert_eq!(first_image_mime(types.lines()), Some("image/png"));
        assert_eq!(first_image_mime(["text/plain"].into_iter()), None);
    }

    #[test]
    fn missing_binary_is_not_found() {
        match command_stdout("whycodes-no-such-clipboard-bin", &[], TIMEOUT) {
            Err(RunErr::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn stub_roundtrip() {
        let got = with_stub(Ok(PromptClipboard::Empty), read_for_prompt).unwrap();
        assert_eq!(got, PromptClipboard::Empty);
        let got = with_stub(Err("boom".into()), read_for_prompt).unwrap_err();
        assert_eq!(got, "boom");
    }
}
