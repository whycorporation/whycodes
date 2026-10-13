use super::*;

fn usage(input: u64, created: Option<u64>, read: Option<u64>) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: 10,
        cache_creation_input_tokens: created,
        cache_read_input_tokens: read,
    }
}

#[test]
fn reports_a_drop_after_a_cached_step() {
    let note = cache_miss_notice(46_000, &usage(200, Some(48_200), None)).unwrap();
    assert_eq!(
        note,
        "Prompt cache miss: 48.4k tokens sent uncached (last step read 46.0k from cache)"
    );
    // OpenAI-style: the uncached prompt is all in `input_tokens`.
    assert!(cache_miss_notice(10_000, &usage(9_000, None, Some(100))).is_some());
}

#[test]
fn stays_quiet_when_nothing_was_lost() {
    // Previous step barely used the cache.
    assert_eq!(cache_miss_notice(500, &usage(40_000, None, None)), None);
    // Still mostly cached.
    assert_eq!(
        cache_miss_notice(40_000, &usage(300, None, Some(30_000))),
        None
    );
    // Tiny prompt this step.
    assert_eq!(cache_miss_notice(40_000, &usage(900, None, None)), None);
    assert_eq!(tokens(999), "999");
}
