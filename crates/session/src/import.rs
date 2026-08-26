//! Import transcripts from whycodes shares and other harnesses.
//!
//! Best-effort: unknown parts become text. Tools do not replay.

use whycodes_core::types::{Message, MessageContent, Role};

/// Source format. `Auto` peeks at the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Auto,
    WhyCodes,
    Claude,
    Codex,
    OpenCode,
    Pi,
}

impl ImportKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "whycodes" | "native" => Self::WhyCodes,
            "claude" | "claude-code" => Self::Claude,
            "codex" => Self::Codex,
            "opencode" => Self::OpenCode,
            "pi" => Self::Pi,
            _ => Self::Auto,
        }
    }
}

/// Parse `raw` into native messages.
pub fn import_messages(raw: &str, kind: ImportKind) -> anyhow::Result<Vec<Message>> {
    let kind = if kind == ImportKind::Auto {
        detect(raw)
    } else {
        kind
    };
    let msgs = match kind {
        ImportKind::Auto | ImportKind::WhyCodes => parse_whycodes(raw)?,
        ImportKind::Claude => parse_claude(raw)?,
        ImportKind::Codex => parse_codex(raw)?,
        ImportKind::OpenCode => parse_opencode(raw)?,
        ImportKind::Pi => parse_pi(raw)?,
    };
    if msgs.is_empty() {
        anyhow::bail!("no user/assistant messages found in import");
    }
    Ok(msgs)
}

fn detect(raw: &str) -> ImportKind {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        if v.get("info").is_some() && v.get("messages").is_some() {
            return ImportKind::OpenCode;
        }
        if v.get("id").is_some() && v.get("messages").is_some() {
            return ImportKind::WhyCodes;
        }
        if v.get("session").is_some() && v.get("messages").is_some() {
            return ImportKind::WhyCodes;
        }
    }
    let Some(first) = first_jsonl_object(raw) else {
        return ImportKind::WhyCodes;
    };
    let ty = first.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "session_meta" || ty.starts_with("event") || first.get("payload").is_some() {
        return ImportKind::Codex;
    }
    if first.get("type").is_some() && first.get("message").is_some() {
        return ImportKind::Claude;
    }
    if first.get("role").is_some() && first.get("content").is_some() {
        return ImportKind::Pi;
    }
    ImportKind::WhyCodes
}

fn first_jsonl_object(raw: &str) -> Option<serde_json::Value> {
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str(line) {
            return Some(v);
        }
    }
    None
}

fn parse_whycodes(raw: &str) -> anyhow::Result<Vec<Message>> {
    let v: serde_json::Value = serde_json::from_str(raw)?;
    if let Some(arr) = v.get("messages").and_then(|m| m.as_array()) {
        return Ok(arr.iter().filter_map(value_to_message).collect());
    }
    if let Some(arr) = v.as_array() {
        return Ok(arr.iter().filter_map(value_to_message).collect());
    }
    anyhow::bail!("not a whycodes session JSON (expected messages array)")
}

fn parse_opencode(raw: &str) -> anyhow::Result<Vec<Message>> {
    parse_whycodes(raw)
}

fn parse_claude(raw: &str) -> anyhow::Result<Vec<Message>> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg = v.get("message").unwrap_or(&v);
        if let Some(m) = value_to_message(msg) {
            out.push(m);
        }
    }
    Ok(out)
}

fn parse_codex(raw: &str) -> anyhow::Result<Vec<Message>> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = v.get("payload").unwrap_or(&v);
        let ty = payload
            .get("type")
            .or_else(|| v.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if let Some(text) = payload
            .get("message")
            .and_then(|m| m.as_str())
            .or_else(|| payload.get("text").and_then(|t| t.as_str()))
        {
            let role = if ty.contains("user") {
                Role::User
            } else if ty.contains("agent") || ty.contains("assistant") {
                Role::Assistant
            } else {
                continue;
            };
            out.push(
                Message {
                    role,
                    content: MessageContent::text(text),
                    tool_call_id: None,
                    name: None,
                    created_at: None,
                }
                .stamp(),
            );
        } else if let Some(m) = value_to_message(payload) {
            out.push(m);
        }
    }
    Ok(out)
}

fn parse_pi(raw: &str) -> anyhow::Result<Vec<Message>> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(m) = value_to_message(&v) {
            out.push(m);
        }
    }
    Ok(out)
}

fn value_to_message(v: &serde_json::Value) -> Option<Message> {
    let role = v.get("role").and_then(|r| r.as_str())?;
    let role = match role {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => return None,
        "tool" => Role::Tool,
        _ => return None,
    };
    let content = v.get("content")?;
    let content = if let Some(s) = content.as_str() {
        MessageContent::text(s)
    } else if let Some(arr) = content.as_array() {
        let text = arr
            .iter()
            .filter_map(|b| {
                b.get("text")
                    .and_then(|t| t.as_str())
                    .or_else(|| b.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            MessageContent::text(content.to_string())
        } else {
            MessageContent::text(text)
        }
    } else {
        MessageContent::text(content.to_string())
    };
    Some(
        Message {
            role,
            content,
            tool_call_id: v
                .get("tool_call_id")
                .and_then(|t| t.as_str())
                .map(str::to_string),
            name: v.get("name").and_then(|n| n.as_str()).map(str::to_string),
            created_at: None,
        }
        .stamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_jsonl() {
        let raw = r#"
{"type":"user","message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi there"}]}}
"#;
        let msgs = import_messages(raw, ImportKind::Auto).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[0].content.as_text(), Some("hello"));
        assert_eq!(msgs[1].content.as_text(), Some("hi there"));
    }

    #[test]
    fn whycodes_share() {
        let raw = r#"{"id":"abc","title":"t","messages":[{"role":"user","content":"ping"}]}"#;
        let msgs = import_messages(raw, ImportKind::Auto).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content.as_text(), Some("ping"));
    }

    #[test]
    fn codex_jsonl() {
        let raw = r#"
{"type":"session_meta","payload":{"id":"x"}}
{"type":"event_msg","payload":{"type":"user_message","message":"fix the bug"}}
{"type":"event_msg","payload":{"type":"agent_message","message":"looking"}}
"#;
        let msgs = import_messages(raw, ImportKind::Auto).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn import_kind_parse_covers_aliases_and_fallback() {
        assert_eq!(ImportKind::parse(" native "), ImportKind::WhyCodes);
        assert_eq!(ImportKind::parse("whycodes"), ImportKind::WhyCodes);
        assert_eq!(ImportKind::parse("CLAUDE-CODE"), ImportKind::Claude);
        assert_eq!(ImportKind::parse("codex"), ImportKind::Codex);
        assert_eq!(ImportKind::parse("opencode"), ImportKind::OpenCode);
        assert_eq!(ImportKind::parse("pi"), ImportKind::Pi);
        assert_eq!(ImportKind::parse("unknown"), ImportKind::Auto);
    }

    #[test]
    fn explicit_import_kinds_and_empty_errors() {
        let native = r#"[{"role":"user","content":"hello"}]"#;
        assert_eq!(
            import_messages(native, ImportKind::WhyCodes).unwrap().len(),
            1
        );
        assert_eq!(
            import_messages(native, ImportKind::OpenCode).unwrap().len(),
            1
        );

        let claude = "not json\n{\"role\":\"assistant\",\"content\":\"answer\"}";
        assert_eq!(
            import_messages(claude, ImportKind::Claude).unwrap().len(),
            1
        );
        let pi = "\ninvalid\n{\"role\":\"user\",\"content\":\"question\"}";
        assert_eq!(import_messages(pi, ImportKind::Pi).unwrap().len(), 1);

        let err = import_messages(r#"{"messages":[]}"#, ImportKind::WhyCodes).unwrap_err();
        assert!(err.to_string().contains("no user/assistant"));
        assert!(import_messages("{}", ImportKind::WhyCodes).is_err());
        assert!(import_messages("not json", ImportKind::WhyCodes).is_err());
    }

    #[test]
    fn auto_detects_all_supported_shapes() {
        let cases = [
            (
                r#"{"info":{},"messages":[{"role":"user","content":"o"}]}"#,
                "o",
            ),
            (
                r#"{"session":{},"messages":[{"role":"user","content":"w"}]}"#,
                "w",
            ),
            (
                "{\"type\":\"event_custom\",\"payload\":{}}\n{\"payload\":{\"type\":\"user_message\",\"text\":\"c\"}}",
                "c",
            ),
            (
                "{\"payload\":{}}\n{\"payload\":{\"type\":\"assistant_message\",\"message\":\"a\"}}",
                "a",
            ),
            ("{\"role\":\"user\",\"content\":\"p\"}", "p"),
        ];
        for (raw, expected) in cases {
            let messages = import_messages(raw, ImportKind::Auto).unwrap();
            assert_eq!(messages[0].content.as_text(), Some(expected));
        }
    }

    #[test]
    fn codex_skips_noise_and_accepts_native_payload_messages() {
        let raw = r#"

not-json
{"type":"event_msg","payload":{"type":"irrelevant","message":"skip"}}
{"type":"user_message","text":"direct user"}
{"payload":{"role":"assistant","content":"native assistant"}}
{"payload":{"role":"system","content":"hidden"}}
"#;
        let msgs = import_messages(raw, ImportKind::Codex).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn value_conversion_handles_roles_content_shapes_and_metadata() {
        let raw = r#"[
          {"role":"tool","content":[{"text":"one"},"two",{"other":1}],"tool_call_id":"call","name":"read"},
          {"role":"assistant","content":[{"image":"x"}]},
          {"role":"user","content":{"nested":true}},
          {"role":"system","content":"skip"},
          {"role":"unknown","content":"skip"},
          {"content":"skip"},
          {"role":"user"}
        ]"#;
        let msgs = import_messages(raw, ImportKind::WhyCodes).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::Tool);
        assert_eq!(msgs[0].content.as_text(), Some("one\ntwo"));
        assert_eq!(msgs[0].tool_call_id.as_deref(), Some("call"));
        assert_eq!(msgs[0].name.as_deref(), Some("read"));
        assert_eq!(msgs[1].content.as_text(), Some("[{\"image\":\"x\"}]"));
        assert_eq!(msgs[2].content.as_text(), Some("{\"nested\":true}"));
    }

    #[test]
    fn direct_parsers_cover_remaining_detection_and_loop_fallbacks() {
        assert_eq!(detect("\ninvalid\n{}"), ImportKind::WhyCodes);
        assert_eq!(
            detect("{\"unrecognized\":true}\nnot-json"),
            ImportKind::WhyCodes
        );
        assert_eq!(detect("{}\n"), ImportKind::WhyCodes);
        assert_eq!(detect("not-json\n{}"), ImportKind::WhyCodes);
        assert_eq!(detect("not-json-only"), ImportKind::WhyCodes);
        assert!(first_jsonl_object("\ninvalid\nalso-invalid").is_none());
        assert_eq!(
            import_messages(
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"a\"}}",
                ImportKind::Claude,
            )
            .unwrap()[0]
                .role,
            Role::Assistant
        );
        assert_eq!(
            import_messages("{\"role\":\"tool\",\"content\":\"r\"}\n", ImportKind::Pi,).unwrap()[0]
                .role,
            Role::Tool
        );
        for kind in [ImportKind::Claude, ImportKind::Codex, ImportKind::Pi] {
            assert!(import_messages("\ninvalid", kind).is_err());
        }
    }
}
