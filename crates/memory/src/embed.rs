//! Lightweight hashing embedder (no ONNX / network).
//!
//! Token + character n-gram features are hashed into a fixed-dim f32 vector
//! and L2-normalized for cosine similarity. Good enough for short durable
//! facts; MiniLM can replace this later without changing the store API.

/// Default embedding dimension (must match stored BLOBs).
pub const DEFAULT_DIM: usize = 256;

/// Embed `text` into a unit-length vector of length `dim`.
pub fn embed(text: &str, dim: usize) -> Vec<f32> {
    let dim = dim.max(8);
    let mut v = vec![0.0f32; dim];
    let lower = text.to_lowercase();

    // Word tokens
    for tok in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if tok.len() < 2 {
            continue;
        }
        accumulate(&mut v, tok, 1.0);
        // Prefixes help partial matches ("cargo" ↔ "cargo check")
        // Must respect UTF-8 char boundaries (Turkish ı, etc.).
        if tok.chars().count() > 4 {
            let prefix: String = tok.chars().take(4).collect();
            accumulate(&mut v, &prefix, 0.35);
        }
    }

    // Character trigrams over the full lowercase string (spaces kept as separators)
    let chars: Vec<char> = lower.chars().filter(|c| !c.is_control()).collect();
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            let tri: String = w.iter().collect();
            if tri.chars().all(|c| c.is_whitespace()) {
                continue;
            }
            accumulate(&mut v, &tri, 0.5);
        }
    }

    l2_normalize(&mut v);
    v
}

fn accumulate(v: &mut [f32], feature: &str, weight: f32) {
    let h = hash_feature(feature);
    let idx = (h as usize) % v.len();
    // Signed hashing reduces collision bias
    let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
    v[idx] += weight * sign;
}

fn hash_feature(s: &str) -> u64 {
    // FNV-1a 64-bit
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn l2_normalize(v: &mut [f32]) {
    let mut sum = 0.0f32;
    for x in v.iter() {
        sum += *x * *x;
    }
    if sum <= f32::EPSILON {
        return;
    }
    let inv = sum.sqrt().recip();
    for x in v.iter_mut() {
        *x *= inv;
    }
}

/// Cosine similarity for L2-normalized vectors (dot product).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += *x * *y;
    }
    // Clamp numerical noise
    dot.clamp(-1.0, 1.0)
}

/// Encode f32 slice as little-endian bytes for SQLite BLOB storage.
pub fn encode_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode little-endian f32 BLOB. Returns empty vec on invalid length.
pub fn decode_blob(bytes: &[u8]) -> Vec<f32> {
    if !bytes.len().is_multiple_of(4) {
        return Vec::new();
    }
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_texts_score_higher() {
        let a = embed(
            "use cargo check -p whycodes-cli after Rust edits",
            DEFAULT_DIM,
        );
        let b = embed("remember to run cargo check for the cli crate", DEFAULT_DIM);
        let c = embed("the weather in Istanbul is sunny today", DEFAULT_DIM);
        let rel = cosine(&a, &b);
        let unrel = cosine(&a, &c);
        assert!(rel > unrel, "related={rel} should exceed unrelated={unrel}");
        assert!(rel > 0.15, "related score too low: {rel}");
    }

    #[test]
    fn blob_roundtrip() {
        let v = embed("hello world", 32);
        let blob = encode_blob(&v);
        let back = decode_blob(&blob);
        assert_eq!(v.len(), back.len());
        for (a, b) in v.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn unit_length() {
        let v = embed("unit length check for embeddings", DEFAULT_DIM);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm={norm}");
    }
}
