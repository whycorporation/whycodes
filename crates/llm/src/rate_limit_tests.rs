use super::*;

#[test]
fn test_parse_retry_after_seconds() {
    assert_eq!(parse_retry_after("30"), Duration::from_secs(30));
    assert_eq!(parse_retry_after(" 60 "), Duration::from_secs(60));
}

#[test]
fn test_parse_retry_after_fallback() {
    assert_eq!(parse_retry_after("invalid"), Duration::from_secs(5));
}

#[test]
fn test_is_rate_limited() {
    assert!(is_rate_limited(429));
    assert!(!is_rate_limited(200));
    assert!(!is_rate_limited(500));
}

#[test]
fn test_rate_limiter_acquire_fast() {
    let limiter = RateLimiter::new(1_000_000.0);
    assert_eq!(limiter.acquire(), Duration::ZERO);
}

#[test]
fn test_rate_limiter_pause() {
    let limiter = RateLimiter::new(100.0);
    limiter.pause(Duration::from_secs(1));
    assert!(limiter.is_paused());
    let wait = limiter.acquire();
    assert!(wait > Duration::ZERO);
}

#[test]
fn parse_retry_after_http_date() {
    let future = (chrono::Utc::now() + chrono::Duration::seconds(30))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    let wait = parse_retry_after(&future);
    assert!(wait >= Duration::from_secs(20), "{wait:?}");
    let past = "Wed, 21 Oct 2015 07:28:00 GMT";
    assert_eq!(parse_retry_after(past), Duration::ZERO);
}

#[test]
fn acquire_waits_when_bucket_is_empty_and_pause_expires() {
    let limiter = RateLimiter::new(0.01);
    assert_eq!(limiter.rps(), 0.01);
    assert_eq!(limiter.acquire(), Duration::ZERO);
    let wait = limiter.acquire();
    assert!(wait > Duration::ZERO, "{wait:?}");

    limiter.pause(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(5));
    let _ = limiter.acquire();
    assert!(!limiter.is_paused());
}

#[test]
fn acquire_when_paused_without_deadline_keeps_going() {
    let limiter = RateLimiter::new(100.0);
    limiter.paused.store(true, Ordering::SeqCst);
    let wait = limiter.acquire();
    assert_eq!(wait, Duration::ZERO);
    assert!(limiter.is_paused());
}

#[test]
fn lock_recovers_from_poison() {
    let limiter = RateLimiter::new(100.0);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _tokens = limiter.tokens.lock().unwrap();
        let _last = limiter.last_refill.lock().unwrap();
        let _pause = limiter.pause_until.lock().unwrap();
        panic!("poison rate limiter");
    }));
    let wait = limiter.acquire();
    assert_eq!(wait, Duration::ZERO);
    limiter.pause(Duration::from_millis(1));
    assert!(limiter.is_paused());
}
