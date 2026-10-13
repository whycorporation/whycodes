use super::*;
use serde_json::json;

fn usage(input: u64, output: u64, read: Option<u64>, write: Option<u64>) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: write,
        cache_read_input_tokens: read,
    }
}

#[test]
fn cost_bills_each_bucket_and_falls_back_to_the_input_rate() {
    let price = ModelPrice {
        input: 3.0,
        output: 15.0,
        cache_read: Some(0.3),
        cache_write: Some(3.75),
    };
    let c = price.cost(&usage(1_000_000, 100_000, Some(2_000_000), Some(400_000)));
    assert!((c - (3.0 + 1.5 + 0.6 + 1.5)).abs() < 1e-9, "{c}");

    let plain = ModelPrice {
        cache_read: None,
        cache_write: None,
        ..price
    };
    let c = plain.cost(&usage(0, 0, Some(1_000_000), Some(1_000_000)));
    assert!((c - 6.0).abs() < 1e-9, "{c}");
    assert_eq!(plain.cost(&usage(0, 0, None, None)), 0.0);
}

#[test]
fn openrouter_pricing_is_per_token_strings_or_numbers() {
    let m = json!({ "id": "x", "pricing": {
        "prompt": "0.000003", "completion": 0.000015,
        "input_cache_read": "0.0000003", "input_cache_write": "bad"
    }});
    let p = price_from_model_value(&m).unwrap();
    assert!((p.input - 3.0).abs() < 1e-9);
    assert!((p.output - 15.0).abs() < 1e-9);
    assert!((p.cache_read.unwrap() - 0.3).abs() < 1e-9);
    assert_eq!(p.cache_write, None);

    assert_eq!(price_from_model_value(&json!({ "id": "x" })), None);
    assert_eq!(
        price_from_model_value(&json!({ "pricing": { "prompt": "0.1" } })),
        None,
        "completion is required"
    );
    assert_eq!(
        price_from_model_value(&json!({ "pricing": { "prompt": "-1", "completion": "1" } })),
        None,
        "negative sentinel (OpenRouter auto-router) is not a price"
    );
    assert_eq!(
        price_from_model_value(&json!({ "pricing": { "prompt": true, "completion": "1" } })),
        None
    );
}

#[test]
fn remembered_prices_are_keyed_by_provider_and_model() {
    let p = ModelPrice {
        input: 1.0,
        output: 2.0,
        cache_read: None,
        cache_write: None,
    };
    remember_price("wc-pricing-test", "m1", p);
    assert_eq!(lookup_price("wc-pricing-test", "m1"), Some(p));
    assert_eq!(lookup_price("wc-pricing-test", "m2"), None);
    assert_eq!(lookup_price("other", "m1"), None);
    let newer = ModelPrice { input: 9.0, ..p };
    remember_price("wc-pricing-test", "m1", newer);
    assert_eq!(lookup_price("wc-pricing-test", "m1"), Some(newer));
}

#[test]
fn poisoned_price_table_still_serves() {
    let _ = std::thread::spawn(|| {
        let _guard = PRICES.write().unwrap();
        panic!("poison the price table");
    })
    .join();
    let p = ModelPrice {
        input: 4.0,
        output: 4.0,
        cache_read: None,
        cache_write: None,
    };
    remember_price("wc-poison", "m", p);
    assert_eq!(lookup_price("wc-poison", "m"), Some(p));
}
