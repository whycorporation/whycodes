//! Token estimation — single source for `session` and `llm` (issue #52 C2).
//!
//! `session::estimate_tokens` and `llm::token_counter::chars_to_tokens_fallback`
//! were two copies of `chars.div_ceil(4)` with a subtle mismatch: session
//! returned `0` for empty input while llm returned `max(1)`. Unifying here
//! makes the policy explicit and testable.

/// ~4 Unicode scalars per token (matches `whycodes_llm` fallback family).
///
/// ASCII uses byte length (same as scalar count); non-ASCII walks `chars`.
/// Returns `0` for empty input — callers that need at least 1 should use
/// [`estimate_tokens_at_least_one`].
pub fn estimate_tokens(text: &str) -> usize {
    let n = if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    };
    n.div_ceil(4)
}

/// Same as [`estimate_tokens`] but at least `1` (for `llm::count_tokens`
/// where an empty prompt still costs one token in provider accounting).
pub fn estimate_tokens_at_least_one(text: &str) -> usize {
    estimate_tokens(text).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens_at_least_one(""), 1);
    }

    #[test]
    fn ascii_fast_path() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("a"), 1); // 1.div_ceil(4)=1
        // empty already tested
    }

    #[test]
    fn unicode_counts_scalars() {
        // 4 scalars -> 1 token, 5 -> 2
        assert_eq!(estimate_tokens("🦀🦀🦀🦀"), 1);
        assert_eq!(estimate_tokens("🦀🦀🦀🦀🦀"), 2);
    }

    #[test]
    fn llm_wrapper_max_one() {
        assert_eq!(estimate_tokens_at_least_one("a"), 1);
        assert_eq!(estimate_tokens_at_least_one("abcd"), 1);
        assert_eq!(estimate_tokens_at_least_one("abcde"), 2);
    }
}
