use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn retryable_omniroute_bracket_500_json() {
    let body = r#"{"error":{"message":"[500]: An internal server error occurred","type":"server_error","code":"internal_server_error"}}"#;
    assert!(is_retryable_message(body));
}

#[test]
fn non_retryable_401() {
    assert!(!is_retryable_message(
        "Provider API error (401): Unauthorized"
    ));
}

#[test]
fn backoff_grows_and_caps() {
    let p = RetryPolicy {
        initial_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(1000),
        full_jitter: false,
        ..RetryPolicy::default()
    };
    assert_eq!(p.backoff_for_attempt(1), Duration::from_millis(100));
    assert_eq!(p.backoff_for_attempt(2), Duration::from_millis(200));
    assert_eq!(p.backoff_for_attempt(3), Duration::from_millis(400));
    assert_eq!(p.backoff_for_attempt(10), Duration::from_millis(1000));
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
}

#[tokio::test]
async fn retries_then_succeeds() {
    init_tracing();
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let policy = RetryPolicy::test_fast();
    let out = execute_with_policy(&policy, "test", || {
        let c = c.clone();
        async move {
            let i = c.fetch_add(1, Ordering::SeqCst);
            if i < 2 {
                Err(whycodes_core::Error::llm("API error (503): unavailable"))
            } else {
                Ok(42)
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(out, 42);
    assert_eq!(n.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn does_not_retry_client_errors() {
    init_tracing();
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let policy = RetryPolicy::test_fast();
    let err = execute_with_policy(&policy, "test", || {
        let c = c.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(whycodes_core::Error::llm("API error (400): bad request"))
        }
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("400"));
    assert_eq!(n.load(Ordering::SeqCst), 1);
}

#[test]
fn jitter_and_retry_after_floor() {
    assert_eq!(full_jitter(Duration::ZERO), Duration::ZERO);
    let j = full_jitter(Duration::from_millis(20));
    assert!(j <= Duration::from_millis(20));
    let huge = full_jitter(Duration::from_secs(u64::MAX / 2));
    assert!(huge <= Duration::from_secs(u64::MAX / 2));
    let p = RetryPolicy {
        full_jitter: false,
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
        ..RetryPolicy::default()
    };
    let classified = crate::error_class::classify_message("(429) retry-after: 2");
    let d = p.sleep_duration(1, &classified);
    assert!(d >= Duration::from_secs(2), "{d:?}");
    assert!(is_retryable(&whycodes_core::Error::llm(
        "API error (503): unavailable"
    )));
    assert!(!is_retryable(&whycodes_core::Error::llm(
        "API error (400): bad"
    )));
}

#[tokio::test]
async fn retry_with_backoff_wrapper_and_elapsed_cap() {
    init_tracing();
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let out = retry_with_backoff(
        || {
            let c = c.clone();
            async move {
                let i = c.fetch_add(1, Ordering::SeqCst);
                if i == 0 {
                    Err(whycodes_core::Error::llm("API error (503): unavailable"))
                } else {
                    Ok(7)
                }
            }
        },
        2,
        1,
    )
    .await
    .unwrap();
    assert_eq!(out, 7);

    let policy = RetryPolicy {
        max_retries: 3,
        initial_backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(5),
        max_elapsed: Duration::from_millis(1),
        full_jitter: false,
    };
    let err = execute_with_policy(&policy, "elapsed", || async {
        tokio::time::sleep(Duration::from_millis(2)).await;
        Err::<(), _>(whycodes_core::Error::llm("API error (503): unavailable"))
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("503"));
    log_retry_helpers_for_tests();
}
