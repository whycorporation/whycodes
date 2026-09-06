use super::*;
use serde_json::json;

#[test]
fn write_line_appends_jsonl() {
    let dir = std::env::temp_dir().join(format!(
        "whycodes-usage-dump-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("usage.jsonl");
    write_line(
        &path,
        &json!({"source":"openai_compat","usage":{"prompt_tokens":12,"completion_tokens":4}}),
    );
    let text = std::fs::read_to_string(&path).expect("dump file");
    let _ = std::fs::remove_dir_all(&dir);
    let parsed: Value = serde_json::from_str(text.lines().next().expect("one line")).unwrap();
    assert_eq!(parsed["source"], "openai_compat");
    assert_eq!(parsed["usage"]["prompt_tokens"], 12);
    assert_eq!(parsed["usage"]["completion_tokens"], 4);
}

#[test]
fn dump_skips_non_objects() {
    dump_raw_usage("openai_compat", &Value::Null);
}

#[test]
fn dump_respects_env_destinations() {
    let usage = json!({"prompt_tokens": 1});
    let prev = std::env::var("WHYCODES_USAGE_DUMP").ok();
    unsafe {
        std::env::remove_var("WHYCODES_USAGE_DUMP");
    }
    dump_raw_usage("openai_compat", &usage);

    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", "");
    }
    dump_raw_usage("openai_compat", &usage);
    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", "0");
    }
    dump_raw_usage("openai_compat", &usage);
    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", "-");
    }
    dump_raw_usage("openai_compat", &usage);
    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", "1");
    }
    dump_raw_usage("openai_compat", &usage);

    let dir = std::env::temp_dir().join(format!(
        "whycodes-usage-dump-env-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("usage.jsonl");
    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", path.as_os_str());
    }
    dump_raw_usage("openai_compat", &usage);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(text.contains("openai_compat"), "{text}");

    unsafe {
        std::env::set_var("WHYCODES_USAGE_DUMP", "/");
    }
    dump_raw_usage("openai_compat", &usage);

    match prev {
        Some(v) => unsafe { std::env::set_var("WHYCODES_USAGE_DUMP", v) },
        None => unsafe { std::env::remove_var("WHYCODES_USAGE_DUMP") },
    }
}

#[test]
fn write_line_reports_open_and_write_failures() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    write_line(std::path::Path::new("/"), &json!({"source": "x"}));
    if std::path::Path::new("/dev/full").exists() {
        write_line(std::path::Path::new("/dev/full"), &json!({"source": "x"}));
    }
}
