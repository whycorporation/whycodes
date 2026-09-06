use super::*;
use crate::tool::ToolContext;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct IsolatedHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
    prev_no_memory: Option<std::ffi::OsString>,
}

impl IsolatedHome {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WHYCODES_HOME");
        let prev_no_memory = std::env::var_os("WHYCODES_NO_MEMORY");
        unsafe {
            std::env::set_var("WHYCODES_HOME", dir.path());
            std::env::remove_var("WHYCODES_NO_MEMORY");
        }
        Self {
            _guard: guard,
            dir,
            prev,
            prev_no_memory,
        }
    }
}

impl Drop for IsolatedHome {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("WHYCODES_HOME", v),
                None => std::env::remove_var("WHYCODES_HOME"),
            }
            match &self.prev_no_memory {
                Some(v) => std::env::set_var("WHYCODES_NO_MEMORY", v),
                None => std::env::remove_var("WHYCODES_NO_MEMORY"),
            }
        }
    }
}

#[test]
fn memory_module_loads() {
    assert!(!module_path!().is_empty());
}

#[tokio::test]
async fn execute_write_list_search_delete_and_unknown() {
    let home = IsolatedHome::new();
    let ctx = ToolContext::new(home.dir.path().to_string_lossy());
    let tool = MemoryTool::new();

    let empty = tool
        .execute(serde_json::json!({"action": "list"}), &ctx)
        .await;
    assert!(!empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("No memories"), "{}", empty.content);

    let missing_text = tool
        .execute(serde_json::json!({"action": "write"}), &ctx)
        .await;
    assert!(missing_text.is_error, "{}", missing_text.content);

    let written = tool
        .execute(
            serde_json::json!({"action": "write", "text": "prefer cargo test -p"}),
            &ctx,
        )
        .await;
    assert!(!written.is_error, "{}", written.content);
    assert!(
        written.content.contains("Saved memory"),
        "{}",
        written.content
    );

    let listed = tool
        .execute(serde_json::json!({"action": "list"}), &ctx)
        .await;
    assert!(!listed.is_error, "{}", listed.content);
    assert!(
        listed.content.contains("prefer cargo test -p"),
        "{}",
        listed.content
    );

    let found = tool
        .execute(
            serde_json::json!({
                "action": "search",
                "text": "prefer cargo test -p"
            }),
            &ctx,
        )
        .await;
    assert!(!found.is_error, "{}", found.content);
    assert!(
        found.content.contains("prefer cargo test -p"),
        "{}",
        found.content
    );

    let miss_search = tool
        .execute(
            serde_json::json!({
                "action": "search",
                "text": "zzzz-no-such-memory-token-xyz"
            }),
            &ctx,
        )
        .await;
    assert!(!miss_search.is_error, "{}", miss_search.content);
    assert!(
        miss_search.content.contains("No matching"),
        "{}",
        miss_search.content
    );

    let empty_search = tool
        .execute(serde_json::json!({"action": "search"}), &ctx)
        .await;
    assert!(empty_search.is_error, "{}", empty_search.content);

    let empty_delete = tool
        .execute(serde_json::json!({"action": "delete"}), &ctx)
        .await;
    assert!(empty_delete.is_error, "{}", empty_delete.content);

    let no_id = tool
        .execute(
            serde_json::json!({"action": "delete", "id": "deadbeef"}),
            &ctx,
        )
        .await;
    assert!(!no_id.is_error, "{}", no_id.content);
    assert!(
        no_id.content.contains("No memory matching"),
        "{}",
        no_id.content
    );

    let id = listed
        .content
        .lines()
        .find_map(|l| {
            l.split(['[', ']'])
                .nth(1)
                .map(str::to_string)
                .filter(|s| !s.is_empty() && s != "memories:")
        })
        .expect("listed memory id");
    let deleted = tool
        .execute(serde_json::json!({"action": "delete", "id": id}), &ctx)
        .await;
    assert!(!deleted.is_error, "{}", deleted.content);
    assert!(deleted.content.contains("Deleted"), "{}", deleted.content);

    let unknown = tool
        .execute(serde_json::json!({"action": "nope"}), &ctx)
        .await;
    assert!(unknown.is_error, "{}", unknown.content);
    assert!(
        unknown.content.contains("unknown action"),
        "{}",
        unknown.content
    );

    let disabled = {
        unsafe { std::env::set_var("WHYCODES_NO_MEMORY", "1") };
        tool.execute(serde_json::json!({"action": "list"}), &ctx)
            .await
    };
    assert!(disabled.is_error, "{}", disabled.content);
    assert!(
        disabled.content.contains("disabled"),
        "{}",
        disabled.content
    );
}

#[tokio::test]
async fn learn_index_code_search_and_metadata() {
    let home = IsolatedHome::new();
    std::fs::create_dir_all(home.dir.path().join("src")).unwrap();
    std::fs::write(
        home.dir.path().join("src/lib.rs"),
        "/// Memory service for semantic recall.\npub fn remember_fact(s: &str) {}\n",
    )
    .unwrap();
    let ctx = ToolContext::new(home.dir.path().to_string_lossy());
    let tool = MemoryTool;
    assert_eq!(tool.name(), "memory");
    assert!(!tool.description().is_empty());
    assert_eq!(tool.parameters()["required"][0], "action");

    let empty_learn = tool
        .execute(serde_json::json!({"action": "learn"}), &ctx)
        .await;
    assert!(empty_learn.is_error, "{}", empty_learn.content);

    let learned = tool
        .execute(
            serde_json::json!({"action": "learn", "text": "always run crate tests"}),
            &ctx,
        )
        .await;
    assert!(!learned.is_error, "{}", learned.content);
    assert!(
        learned.content.contains("Lesson stored"),
        "{}",
        learned.content
    );

    let empty_code = tool
        .execute(serde_json::json!({"action": "code_search"}), &ctx)
        .await;
    assert!(empty_code.is_error, "{}", empty_code.content);

    let before_index = tool
        .execute(
            serde_json::json!({"action": "code_search", "text": "remember_fact"}),
            &ctx,
        )
        .await;
    assert!(!before_index.is_error, "{}", before_index.content);
    assert!(
        before_index.content.contains("No code hits")
            || before_index.content.contains("remember_fact"),
        "{}",
        before_index.content
    );

    let indexed = tool
        .execute(serde_json::json!({"action": "index", "limit": 5}), &ctx)
        .await;
    assert!(!indexed.is_error, "{}", indexed.content);
    assert!(indexed.content.contains("Indexed"), "{}", indexed.content);

    let hits = tool
        .execute(
            serde_json::json!({"action": "code_search", "text": "remember_fact"}),
            &ctx,
        )
        .await;
    assert!(!hits.is_error, "{}", hits.content);

    for flag in ["true", "yes", "on"] {
        unsafe { std::env::set_var("WHYCODES_NO_MEMORY", flag) };
        let disabled = tool
            .execute(serde_json::json!({"action": "list"}), &ctx)
            .await;
        assert!(disabled.is_error, "{flag}: {}", disabled.content);
        unsafe { std::env::remove_var("WHYCODES_NO_MEMORY") };
    }
}
