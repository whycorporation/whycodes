//! Optional MiniLM ONNX embedder (cargo feature `onnx`).
//!
//! Without the feature, [`try_embed`] returns `None` and callers use the
//! hashing embedder. With `--features onnx`, downloads MiniLM into
//! `{data_dir}/models/minilm/` on first use and runs mean-pooled inference.

use std::path::{Path, PathBuf};

/// all-MiniLM-L6-v2 hidden size.
pub const ONNX_DIM: usize = 384;

pub fn onnx_available() -> bool {
    cfg!(feature = "onnx")
}

pub fn model_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join("minilm")
}

/// Ensure model + tokenizer files exist (download if needed).
pub fn ensure_model(data_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = model_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let onnx_path = dir.join("model.onnx");
    let tok_path = dir.join("tokenizer.json");

    const ONNX_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
    const TOK_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

    if !onnx_path.exists() {
        download(ONNX_URL, &onnx_path)?;
    }
    if !tok_path.exists() {
        download(TOK_URL, &tok_path)?;
    }
    Ok((onnx_path, tok_path))
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

    let ids_arr = ndarray::Array2::from_shape_vec((1, len), ids)?;
    let mask_arr = ndarray::Array2::from_shape_vec((1, len), mask)?;
    let type_arr = ndarray::Array2::from_shape_vec((1, len), type_ids)?;

    // Try 3-input then 2-input signatures used by different MiniLM ONNX exports.
    let result = model
        .run(tvec!(
            ids_arr.clone().into_tensor().into(),
            mask_arr.clone().into_tensor().into(),
            type_arr.into_tensor().into()
        ))
        .or_else(|_| {
            model.run(tvec!(
                ids_arr.into_tensor().into(),
                mask_arr.into_tensor().into()
            ))
        })?;

    let output = result[0].to_array_view::<f32>()?;
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
