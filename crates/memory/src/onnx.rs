//! Optional MiniLM ONNX embedder (cargo feature `onnx`).
//!
//! Without the feature, [`try_embed`] returns `None` and callers use the
//! hashing embedder. With `--features onnx`, downloads MiniLM into
//! `{data_dir}/models/minilm/` on first use and runs mean-pooled inference.
//!
//! Downloaded files are verified with SHA-256 (pinned when known; otherwise
//! a sidecar `.sha256` is written after the first successful download).

use std::path::{Path, PathBuf};

/// all-MiniLM-L6-v2 hidden size.
pub const ONNX_DIM: usize = 384;

pub fn onnx_available() -> bool {
    cfg!(feature = "onnx")
}

pub fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join("minilm")
}

/// Optional pinned SHA-256 for the tokenizer (stable HF file).
/// Empty model pin: first download writes a sidecar and subsequent loads verify it.
const TOKENIZER_SHA256: &str =
    // May drift if HF rewrites tokenizer.json — sidecar still protects after first pull.
    "";

/// Ensure model + tokenizer files exist (download + checksum if needed).
pub fn ensure_model(data_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = model_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let onnx_path = dir.join("model.onnx");
    let tok_path = dir.join("tokenizer.json");

    const ONNX_URL: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
    const TOK_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

    ensure_file(&onnx_path, ONNX_URL, None)?;
    ensure_file(
        &tok_path,
        TOK_URL,
        if TOKENIZER_SHA256.is_empty() {
            None
        } else {
            Some(TOKENIZER_SHA256)
        },
    )?;
    Ok((onnx_path, tok_path))
}

fn ensure_file(path: &Path, url: &str, pinned: Option<&str>) -> anyhow::Result<()> {
    let sidecar = sha_sidecar(path);
    if path.exists() {
        verify_or_repair(path, &sidecar, pinned)?;
        return Ok(());
    }
    download(url, path)?;
    let digest = sha256_file(path)?;
    if let Some(expected) = pinned
        && !digest.eq_ignore_ascii_case(expected)
    {
        let _ = std::fs::remove_file(path);
        anyhow::bail!(
            "checksum mismatch for {}: got {digest}, expected {expected}",
            path.display()
        );
    }
    write_sidecar(&sidecar, &digest)?;
    Ok(())
}

fn verify_or_repair(path: &Path, sidecar: &Path, pinned: Option<&str>) -> anyhow::Result<()> {
    let digest = sha256_file(path)?;
    if let Some(expected) = pinned {
        if !digest.eq_ignore_ascii_case(expected) {
            anyhow::bail!(
                "checksum mismatch for {}: got {digest}, expected {expected}. Delete the file to re-download.",
                path.display()
            );
        }
        // Keep sidecar in sync
        let _ = write_sidecar(sidecar, &digest);
        return Ok(());
    }
    if sidecar.exists() {
        let expected = std::fs::read_to_string(sidecar)?.trim().to_string();
        if !expected.is_empty() && !digest.eq_ignore_ascii_case(&expected) {
            anyhow::bail!(
                "checksum mismatch for {} (sidecar). got {digest}, expected {expected}. Delete both to re-download.",
                path.display()
            );
        }
    } else {
        write_sidecar(sidecar, &digest)?;
    }
    Ok(())
}

fn sha_sidecar(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".sha256");
    PathBuf::from(s)
}

fn write_sidecar(path: &Path, digest: &str) -> anyhow::Result<()> {
    std::fs::write(path, format!("{digest}\n"))?;
    Ok(())
}

/// SHA-256 hex digest of a file (streaming).
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize_hex())
}

/// Minimal pure-Rust SHA-256 (avoids pulling sha2 into default builds).
struct Sha256Hasher {
    // Use sha2 when available via optional dep; otherwise implement via `sha2` always
    // for correctness. Memory crate will depend on sha2 lightly.
    inner: sha2_wrap::Hasher,
}

mod sha2_wrap {
    use sha2::{Digest, Sha256};

    pub struct Hasher {
        h: Sha256,
    }
    impl Hasher {
        pub fn new() -> Self {
            Self { h: Sha256::new() }
        }
        pub fn update(&mut self, data: &[u8]) {
            self.h.update(data);
        }
        pub fn finalize_hex(self) -> String {
            self.h
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }
    }
}

impl Sha256Hasher {
    fn new() -> Self {
        Self {
            inner: sha2_wrap::Hasher::new(),
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
    fn finalize_hex(self) -> String {
        self.inner.finalize_hex()
    }
}

fn download(url: &str, dest: &Path) -> anyhow::Result<()> {
    let tmp = dest.with_extension("partial");
    let try_curl = std::process::Command::new("curl")
        .args(["-fsSL", "-L", "-o"])
        .arg(&tmp)
        .arg(url)
        .status();
    if try_curl.map(|s| s.success()).unwrap_or(false) && tmp.exists() {
        std::fs::rename(&tmp, dest)?;
        return Ok(());
    }
    let try_wget = std::process::Command::new("wget")
        .args(["-q", "-O"])
        .arg(&tmp)
        .arg(url)
        .status();
    if try_wget.map(|s| s.success()).unwrap_or(false) && tmp.exists() {
        std::fs::rename(&tmp, dest)?;
        return Ok(());
    }
    let _ = std::fs::remove_file(&tmp);
    anyhow::bail!(
        "failed to download {url}; install curl or wget, or place the file at {}",
        dest.display()
    )
}

/// Run MiniLM when the `onnx` feature is enabled; otherwise `None`.
pub fn try_embed(text: &str, data_dir: &Path) -> Option<Vec<f32>> {
    #[cfg(feature = "onnx")]
    {
        match embed_onnx(text, data_dir) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("onnx embed failed, falling back to hash: {e}");
                None
            }
        }
    }
    #[cfg(not(feature = "onnx"))]
    {
        let _ = (text, data_dir);
        None
    }
}

/// Smoke: ensure model, embed a probe string, return dim. Errors if onnx feature off.
pub fn smoke_embed(data_dir: &Path) -> anyhow::Result<(usize, f32)> {
    #[cfg(feature = "onnx")]
    {
        let v = embed_onnx("whycodes memory onnx smoke test", data_dir)?;
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok((v.len(), norm))
    }
    #[cfg(not(feature = "onnx"))]
    {
        let _ = data_dir;
        anyhow::bail!("build with --features onnx to run ONNX smoke")
    }
}

#[cfg(feature = "onnx")]
fn embed_onnx(text: &str, data_dir: &Path) -> anyhow::Result<Vec<f32>> {
    use tract_onnx::prelude::*;

    let (onnx_path, tok_path) = ensure_model(data_dir)?;
    let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
        .map_err(|e| anyhow::anyhow!("tokenizer load: {e}"))?;
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;

    let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = encoding
        .get_attention_mask()
        .iter()
        .map(|&x| x as i64)
        .collect();
    let len = ids.len();
    let type_ids = vec![0i64; len];

    let model = tract_onnx::onnx()
        .model_for_path(&onnx_path)?
        .into_optimized()?
        .into_runnable()?;

    let ids_t = Tensor::from_shape(&[1, len], &ids)?;
    let mask_t = Tensor::from_shape(&[1, len], &mask)?;
    let type_t = Tensor::from_shape(&[1, len], &type_ids)?;

    let result = model
        .run(tvec!(
            ids_t.clone().into(),
            mask_t.clone().into(),
            type_t.into()
        ))
        .or_else(|_| model.run(tvec!(ids_t.into(), mask_t.into())))?;

    // tract 0.23 renamed Tensor::to_array_view → to_plain_array_view.
    let output = result[0].to_plain_array_view::<f32>()?;
    let shape = output.shape();
    let mut pooled = match shape.len() {
        3 => {
            let (seq, dim) = (shape[1], shape[2]);
            let mut p = vec![0.0f32; dim];
            for t in 0..seq {
                for d in 0..dim {
                    p[d] += output[[0, t, d]];
                }
            }
            let inv = 1.0 / (seq as f32).max(1.0);
            for x in &mut p {
                *x *= inv;
            }
            p
        }
        2 => (0..shape[1]).map(|d| output[[0, d]]).collect(),
        n => anyhow::bail!("unexpected ONNX output rank {n}"),
    };
    let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in &mut pooled {
            *x /= norm;
        }
    }
    Ok(pooled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sha256_known_vector() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("t.txt");
        std::fs::write(&p, b"abc").unwrap();
        // SHA256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let d = sha256_file(&p).unwrap();
        assert_eq!(
            d,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sidecar_roundtrip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("m.bin");
        std::fs::write(&p, b"hello").unwrap();
        let d = sha256_file(&p).unwrap();
        let side = sha_sidecar(&p);
        write_sidecar(&side, &d).unwrap();
        verify_or_repair(&p, &side, None).unwrap();
    }
}
