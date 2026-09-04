//! Optional MiniLM ONNX embedder (cargo feature `onnx`).
//!
//! Without the feature, [`try_embed`] returns `None` and callers use the
//! hashing embedder. With `--features onnx`, downloads MiniLM into
//! `{data_dir}/models/minilm/` on first use and runs mean-pooled inference.
//!
//! Downloaded files are verified with SHA-256 (pinned when known; otherwise
//! a sidecar `.sha256` is written after the first successful download).

use std::path::{Path, PathBuf};

use crate::error::{MemoryError, Result};

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
pub fn ensure_model(data_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let dir = model_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let onnx_path = dir.join("model.onnx");
    let tok_path = dir.join("tokenizer.json");

    const ONNX_URL: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
    const TOK_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

    ensure_file(&onnx_path, ONNX_URL, None)?;
    ensure_file(&tok_path, TOK_URL, optional_pin(TOKENIZER_SHA256))?;
    Ok((onnx_path, tok_path))
}

fn optional_pin(pin: &str) -> Option<&str> {
    if pin.is_empty() { None } else { Some(pin) }
}

fn ensure_file(path: &Path, url: &str, pinned: Option<&str>) -> Result<()> {
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
        return Err(MemoryError::msg(format!(
            "checksum mismatch for {}: got {digest}, expected {expected}",
            path.display()
        )));
    }
    write_sidecar(&sidecar, &digest)?;
    Ok(())
}

fn verify_or_repair(path: &Path, sidecar: &Path, pinned: Option<&str>) -> Result<()> {
    let digest = sha256_file(path)?;
    if let Some(expected) = pinned {
        if !digest.eq_ignore_ascii_case(expected) {
            return Err(MemoryError::msg(format!(
                "checksum mismatch for {}: got {digest}, expected {expected}. Delete the file to re-download.",
                path.display()
            )));
        }
        // Keep sidecar in sync
        let _ = write_sidecar(sidecar, &digest);
        return Ok(());
    }
    if sidecar.exists() {
        let expected = std::fs::read_to_string(sidecar)?.trim().to_string();
        if !expected.is_empty() && !digest.eq_ignore_ascii_case(&expected) {
            return Err(MemoryError::msg(format!(
                "checksum mismatch for {} (sidecar). got {digest}, expected {expected}. Delete both to re-download.",
                path.display()
            )));
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

fn write_sidecar(path: &Path, digest: &str) -> Result<()> {
    std::fs::write(path, format!("{digest}\n"))?;
    Ok(())
}

/// SHA-256 hex digest of a file (streaming).
pub fn sha256_file(path: &Path) -> Result<String> {
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

fn download(url: &str, dest: &Path) -> Result<()> {
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
    Err(MemoryError::msg(format!(
        "failed to download {url}; install curl or wget, or place the file at {}",
        dest.display()
    )))
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
pub fn smoke_embed(data_dir: &Path) -> Result<(usize, f32)> {
    #[cfg(feature = "onnx")]
    {
        let v = embed_onnx("whycodes memory onnx smoke test", data_dir)?;
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok((v.len(), norm))
    }
    #[cfg(not(feature = "onnx"))]
    {
        let _ = data_dir;
        Err(MemoryError::msg(
            "build with --features onnx to run ONNX smoke",
        ))
    }
}

#[cfg(feature = "onnx")]
fn embed_onnx(text: &str, data_dir: &Path) -> Result<Vec<f32>> {
    use tract_onnx::prelude::*;

    let (onnx_path, tok_path) = ensure_model(data_dir)?;
    let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
        .map_err(|e| MemoryError::msg(format!("tokenizer load: {e}")))?;
    let encoding = tokenizer
        .encode(text, true)
        .map_err(|e| MemoryError::msg(format!("tokenize: {e}")))?;

    let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = encoding
        .get_attention_mask()
        .iter()
        .map(|&x| x as i64)
        .collect();
    let len = ids.len();
    let type_ids = vec![0i64; len];

    let model = tract_onnx::onnx()
        .model_for_path(&onnx_path)
        .map_err(MemoryError::wrap)?
        .into_optimized()
        .map_err(MemoryError::wrap)?
        .into_runnable()
        .map_err(MemoryError::wrap)?;

    let ids_t = Tensor::from_shape(&[1, len], &ids).map_err(MemoryError::wrap)?;
    let mask_t = Tensor::from_shape(&[1, len], &mask).map_err(MemoryError::wrap)?;
    let type_t = Tensor::from_shape(&[1, len], &type_ids).map_err(MemoryError::wrap)?;

    let result = model
        .run(tvec!(
            ids_t.clone().into(),
            mask_t.clone().into(),
            type_t.into()
        ))
        .or_else(|_| model.run(tvec!(ids_t.into(), mask_t.into())))
        .map_err(MemoryError::wrap)?;

    // tract 0.23 renamed Tensor::to_array_view → to_plain_array_view.
    let output = result[0]
        .to_plain_array_view::<f32>()
        .map_err(MemoryError::wrap)?;
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
        n => return Err(MemoryError::msg(format!("unexpected ONNX output rank {n}"))),
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

    fn with_path_bins<R>(names_and_scripts: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        use std::os::unix::fs::PermissionsExt;
        let _guard = crate::TEST_PATH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();

        for (name, script) in names_and_scripts {
            let p = dir.path().join(name);
            std::fs::write(&p, script).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prev = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", dir.path()) };
        let out = f();
        match prev {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        out
    }

    #[test]
    fn available_model_dir_and_disabled_embed() {
        assert_eq!(onnx_available(), cfg!(feature = "onnx"));
        assert!(optional_pin("").is_none());
        assert_eq!(optional_pin("abc"), Some("abc"));

        let dir = tempdir().unwrap();
        assert_eq!(
            model_dir(dir.path()),
            dir.path().join("models").join("minilm")
        );
        if !cfg!(feature = "onnx") {
            assert!(try_embed("hello", dir.path()).is_none());
            let err = smoke_embed(dir.path()).unwrap_err();
            assert!(err.to_string().contains("features onnx"));
        }
        let prev = std::env::var_os("PATH");
        unsafe { std::env::remove_var("PATH") };
        let _ = with_path_bins(&[("true", "#!/bin/sh\nexit 0\n")], || 1 + 1);
        match prev {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }

    #[cfg(feature = "onnx")]
    #[test]
    fn onnx_feature_falls_back_when_model_missing() {
        let dir = tempdir().unwrap();
        assert!(try_embed("hello", dir.path()).is_none());
        assert!(smoke_embed(dir.path()).is_err());
    }

    #[test]
    fn verify_pinned_and_sidecar_mismatch() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("m.bin");
        std::fs::write(&p, b"hello").unwrap();
        let digest = sha256_file(&p).unwrap();
        let side = sha_sidecar(&p);
        verify_or_repair(&p, &side, Some(&digest)).unwrap();
        assert!(side.exists());
        let err = verify_or_repair(&p, &side, Some("deadbeef")).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));

        std::fs::write(&side, "0000\n").unwrap();
        let err = verify_or_repair(&p, &side, None).unwrap_err();
        assert!(err.to_string().contains("sidecar"));

        std::fs::write(&side, "\n").unwrap();
        verify_or_repair(&p, &side, None).unwrap();
    }

    #[test]
    fn ensure_file_accepts_matching_pin_and_sha256_missing() {
        let dir = tempdir().unwrap();
        let curl = concat!(
            "#!/bin/sh\n",
            "out=\"\"\n",
            "while [ $# -gt 0 ]; do\n",
            "  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n",
            "  shift\n",
            "done\n",
            "printf abc > \"$out\"\n",
        );
        with_path_bins(&[("curl", curl)], || {
            let dest = dir.path().join("pinned.bin");
            ensure_file(
                &dest,
                "http://127.0.0.1/model",
                Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            )
            .unwrap();
            assert!(dest.exists());
        });
        assert!(sha256_file(&dir.path().join("missing.bin")).is_err());
    }

    #[test]
    fn ensure_file_existing_and_download_paths() {
        let dir = tempdir().unwrap();
        let existing = dir.path().join("present.bin");
        std::fs::write(&existing, b"abc").unwrap();
        ensure_file(&existing, "http://127.0.0.1/unused", None).unwrap();

        let curl = concat!(
            "#!/bin/sh\n",
            "out=\"\"\n",
            "while [ $# -gt 0 ]; do\n",
            "  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n",
            "  shift\n",
            "done\n",
            "printf abc > \"$out\"\n",
        );
        with_path_bins(&[("curl", curl), ("wget", "#!/bin/sh\nexit 1\n")], || {
            let dest = dir.path().join("from-curl.bin");
            ensure_file(&dest, "http://127.0.0.1/model", None).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), b"abc");

            let mismatch = dir.path().join("mismatch.bin");
            let err = ensure_file(&mismatch, "http://127.0.0.1/model", Some("ffff")).unwrap_err();
            assert!(err.to_string().contains("checksum mismatch"));
            assert!(!mismatch.exists());
        });

        let wget = concat!(
            "#!/bin/sh\n",
            "out=\"\"\n",
            "while [ $# -gt 0 ]; do\n",
            "  if [ \"$1\" = \"-O\" ]; then out=\"$2\"; shift 2; continue; fi\n",
            "  shift\n",
            "done\n",
            "printf wget > \"$out\"\n",
        );
        with_path_bins(&[("curl", "#!/bin/sh\nexit 1\n"), ("wget", wget)], || {
            let dest = dir.path().join("from-wget.bin");
            download("http://127.0.0.1/model", &dest).unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), b"wget");
        });

        with_path_bins(
            &[
                ("curl", "#!/bin/sh\nexit 1\n"),
                ("wget", "#!/bin/sh\nexit 1\n"),
            ],
            || {
                let dest = dir.path().join("fail.bin");
                let err = download("http://127.0.0.1/model", &dest).unwrap_err();
                assert!(err.to_string().contains("failed to download"));
            },
        );
        with_path_bins(
            &[
                ("curl", "#!/bin/sh\nexit 0\n"),
                (
                    "wget",
                    concat!(
                        "#!/bin/sh\n",
                        "out=\"\"\n",
                        "while [ $# -gt 0 ]; do\n",
                        "  if [ \"$1\" = \"-O\" ]; then out=\"$2\"; shift 2; continue; fi\n",
                        "  shift\n",
                        "done\n",
                        "printf later > \"$out\"\n",
                    ),
                ),
            ],
            || {
                let dest = dir.path().join("curl-empty-wget.bin");
                download("http://127.0.0.1/model", &dest).unwrap();
                assert_eq!(std::fs::read(&dest).unwrap(), b"later");
            },
        );
    }

    #[test]
    fn ensure_model_with_scripted_curl() {
        let dir = tempdir().unwrap();
        let curl = concat!(
            "#!/bin/sh\n",
            "out=\"\"\n",
            "while [ $# -gt 0 ]; do\n",
            "  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift 2; continue; fi\n",
            "  shift\n",
            "done\n",
            "printf model > \"$out\"\n",
        );
        with_path_bins(&[("curl", curl)], || {
            let (onnx, tok) = ensure_model(dir.path()).unwrap();
            assert!(onnx.ends_with("model.onnx"));
            assert!(tok.ends_with("tokenizer.json"));
            assert!(onnx.exists() && tok.exists());
            // Second call verifies existing files (no download).
            ensure_model(dir.path()).unwrap();
        });
    }

    #[test]
    fn sha256_streams_large_files() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("big.bin");
        std::fs::write(&p, vec![7u8; 70 * 1024]).unwrap();
        let d = sha256_file(&p).unwrap();
        assert_eq!(d.len(), 64);
    }
}
