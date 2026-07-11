use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use whycode_core::types::{
    ContentBlock, LlmRequest, Message, MessageContent, Role,
    SessionInfo, ToolDefinition,
};

/// A conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    pub system_prompt: String,
    pub project_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl Session {
    /// Create a new session
    pub fn new(project_path: PathBuf, system_prompt: String) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!(
                "New session - {}",
                now.format("%Y-%m-%dT%H:%M:%S%.3fZ")
            ),
            messages: Vec::new(),
            system_prompt,
            project_path,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(content.to_string()),
            tool_call_id: None,
            name: None,
        });
        self.touch();
    }

    /// Add an assistant message with content blocks
    pub fn add_assistant_message(&mut self, blocks: Vec<ContentBlock>) {
        self.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            tool_call_id: None,
            name: None,
        });
        self.touch();
    }

    /// Add tool results
    pub fn add_tool_results(&mut self, results: Vec<whycode_core::types::ToolResult>) {
        for result in results {
            self.messages.push(Message {
                role: Role::Tool,
                content: MessageContent::Text(result.content),
                tool_call_id: Some(result.tool_call_id),
                name: None,
            });
        }
        self.touch();
    }

    /// Build an LLM request from the current conversation
    pub fn build_request(
        &self,
        tools: &[ToolDefinition],
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        _thinking: Option<bool>,
    ) -> LlmRequest {
        LlmRequest {
            system: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            thinking: None,
        }
    }

    /// Get session info
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            message_count: self.messages.len(),
            project_path: self.project_path.clone(),
        }
    }

    /// Get conversation as a readable string (for display)
    pub fn conversation_text(&self) -> String {
        let mut out = String::new();
        for msg in &self.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            if let Some(text) = msg.content.as_text() {
                out.push_str(&format!("{}: {}\n", role, text));
            }
        }
        out
    }

    /// Update the system prompt
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = prompt.to_string();
        self.touch();
    }

    /// Estimate token count (simple char-based heuristic)
    pub fn token_count(&self) -> usize {
        self.messages
            .iter()
            .map(|m| match &m.content {
                MessageContent::Text(t) => t.len() / 4,
                MessageContent::Blocks(b) => {
                    b.iter()
                        .map(|block| match block {
                            ContentBlock::Text { text } => text.len() / 4,
                            _ => 100, // rough estimate for non-text blocks
                        })
                        .sum()
                }
            })
            .sum::<usize>()
            + self.system_prompt.len() / 4
    }

    /// Compact the conversation: keep system + last N tokens
    pub fn compact(&mut self, max_tokens: usize) {
        let _target = max_tokens * 3 / 4;
        let _system_tokens = self.system_prompt.len() / 4;

        // Always keep at least last 4 messages
        let keep = 4usize.min(self.messages.len());

        let mut kept_messages: Vec<Message> = Vec::new();
        for msg in self.messages.iter().rev().take(keep).rev() {
            kept_messages.push(msg.clone());
        }

        // Add summary of trimmed messages
        let trimmed_count = self.messages.len() - keep;
        if trimmed_count > 0 {
            let summary = format!(
                "[{} earlier messages trimmed for context management]",
                trimmed_count
            );
            let mut new_messages = vec![Message {
                role: Role::User,
                content: MessageContent::Text(summary),
                tool_call_id: None,
                name: None,
            }];
            new_messages.extend(kept_messages);
            self.messages = new_messages;
        }

        self.touch();
    }

    /// Persist this session and all messages to the SQLite database.
    pub fn save_to_db(&self, db: &whycode_storage::db::Database) -> anyhow::Result<()> {
        // Upsert the session row (INSERT OR REPLACE so repeated saves work).
        db.create_session(
            &self.id,
            &self.title,
            &self.project_path.to_string_lossy(),
        )?;

        // Store each message as a JSON-serialized row.
        for msg in &self.messages {
            let msg_json = serde_json::to_string(msg)?;
            let role_str = serde_json::to_string(&msg.role)?.trim_matches('"').to_string();
            let msg_id = uuid::Uuid::new_v4().to_string();
            db.insert_message(
                &msg_id,
                &self.id,
                &role_str,
                &msg_json,
                msg.tool_call_id.as_deref(),
                msg.name.as_deref(),
            )?;
        }

        Ok(())
    }

    /// Load a session and its messages from the SQLite database by session id.
    pub fn load_from_db(
        db: &whycode_storage::db::Database,
        id: &str,
    ) -> anyhow::Result<Option<Self>> {
        let Some(row) = db.get_session(id)? else {
            return Ok(None);
        };

        let created_at = chrono::DateTime::parse_from_rfc3339(&row.created_at)?
            .with_timezone(&chrono::Utc);
        let updated_at = chrono::DateTime::parse_from_rfc3339(&row.updated_at)?
            .with_timezone(&chrono::Utc);

        let message_rows = db.get_messages(id)?;
        let messages: Vec<whycode_core::types::Message> = message_rows
            .iter()
            .map(|mr| serde_json::from_str(&mr.content))
            .collect::<Result<_, _>>()?;

        Ok(Some(Self {
            id: row.id,
            title: row.title,
            messages,
            system_prompt: String::new(), // system_prompt is not yet persisted
            project_path: std::path::PathBuf::from(row.project_path),
            created_at,
            updated_at,
        }))
    }

    /// Export the session as a shareable JSON file.
    /// Writes to .whycode/shares/{session_id}.json and returns the file path.
    pub fn export_share(&self) -> anyhow::Result<String> {
        let shares_dir = self.project_path.join(".whycode").join("shares");
        std::fs::create_dir_all(&shares_dir)?;

        let filename = format!("{}.json", self.id);
        let share_path = shares_dir.join(&filename);

        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&share_path, json)?;

        // Also write a human-readable Markdown share (OpenCode /export style)
        let md_path = shares_dir.join(format!("{}.md", self.id));
        let _ = std::fs::write(&md_path, self.export_markdown());

        Ok(share_path.to_string_lossy().to_string())
    }

    /// Export conversation as Markdown for sharing / `/export`.
    pub fn export_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", self.title));
        out.push_str(&format!("- **Session:** `{}`\n", self.id));
        out.push_str(&format!("- **Project:** `{}`\n", self.project_path.display()));
        out.push_str(&format!(
            "- **Created:** {}\n\n---\n\n",
            self.created_at.to_rfc3339()
        ));

        for msg in &self.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            out.push_str(&format!("### {role}\n\n"));
            match &msg.content {
                MessageContent::Text(t) => {
                    out.push_str(t);
                    out.push_str("\n\n");
                }
                MessageContent::Blocks(blocks) => {
                    for b in blocks {
                        match b {
                            ContentBlock::Text { text } => {
                                out.push_str(text);
                                out.push_str("\n\n");
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                out.push_str(&format!(
                                    "```tool\n{name}\n{}\n```\n\n",
                                    serde_json::to_string_pretty(input).unwrap_or_default()
                                ));
                            }
                            ContentBlock::ToolResult {
                                content, is_error, ..
                            } => {
                                let tag = if is_error.unwrap_or(false) {
                                    "error"
                                } else {
                                    "result"
                                };
                                out.push_str(&format!("```{tag}\n{content}\n```\n\n"));
                            }
                            ContentBlock::Image { .. } => {
                                out.push_str("*[image]*\n\n");
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Revert the session to a previous state by removing all messages after
    /// the given index. Returns the number of messages removed.
    pub fn revert_to(&mut self, message_index: usize) -> usize {
        if message_index >= self.messages.len() {
            return 0;
        }

        let removed = self.messages.len() - message_index - 1;
        self.messages.truncate(message_index + 1);
        self.touch();
        removed
    }

    /// Replace the entire message list (used by undo/redo).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.touch();
    }

    /// Undo the last user turn: remove from the last user message to the end.
    /// Returns the number of messages removed, or 0 if nothing to undo.
    pub fn undo_last_turn(&mut self) -> usize {
        let last_user = self
            .messages
            .iter()
            .rposition(|m| m.role == Role::User);
        match last_user {
            Some(idx) => {
                let removed = self.messages.len() - idx;
                self.messages.truncate(idx);
                self.touch();
                removed
            }
            None => 0,
        }
    }

    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_project_path() -> PathBuf {
        PathBuf::from("/tmp/test-project")
    }

    fn test_system_prompt() -> String {
        "You are a helpful assistant.".to_string()
    }

    #[test]
    fn test_new_session() {
        let session = Session::new(test_project_path(), test_system_prompt());

        assert!(!session.id.is_empty(), "session id should not be empty");
        assert!(
            session.title.starts_with("New session -"),
            "title should start with 'New session -'"
        );
        assert!(session.messages.is_empty(), "new session should have no messages");
        assert_eq!(session.system_prompt, test_system_prompt());
        assert_eq!(session.project_path, test_project_path());
        assert_eq!(session.created_at, session.updated_at);
    }

    #[test]
    fn test_add_messages() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        session.add_user_message("Hello");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content.as_text(), Some("Hello"));

        session.add_assistant_message(vec![ContentBlock::Text {
            text: "Hi there!".to_string(),
        }]);
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[1].content.as_text(), Some("Hi there!"));

        // updated_at should change after adding messages
        assert!(session.updated_at >= session.created_at);
    }

    #[test]
    fn test_add_tool_results() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        let results = vec![
            whycode_core::types::ToolResult {
                tool_call_id: "call-1".to_string(),
                content: "result 1".to_string(),
                is_error: false,
            },
            whycode_core::types::ToolResult {
                tool_call_id: "call-2".to_string(),
                content: "error result".to_string(),
                is_error: true,
            },
        ];

        session.add_user_message("use tools");
        session.add_tool_results(results);

        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[1].role, Role::Tool);
        assert_eq!(session.messages[1].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(session.messages[1].content.as_text(), Some("result 1"));
        assert_eq!(session.messages[2].role, Role::Tool);
        assert_eq!(session.messages[2].tool_call_id.as_deref(), Some("call-2"));
    }

    #[test]
    fn test_build_request() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("test message");

        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "search tool".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];

        let req = session.build_request(&tools, Some(1024), Some(0.7), None);

        assert_eq!(req.system, test_system_prompt());
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "search");
        assert_eq!(req.max_tokens, Some(1024));
        assert_eq!(req.temperature, Some(0.7));
        assert!(req.top_p.is_none());
        assert!(req.top_k.is_none());
    }

    #[test]
    fn test_build_request_no_tools() {
        let session = Session::new(test_project_path(), test_system_prompt());
        let req = session.build_request(&[], None, None, None);

        assert!(req.tools.is_empty());
        assert_eq!(req.system, test_system_prompt());
        assert!(req.messages.is_empty());
    }

    #[test]
    fn test_token_count() {
        let mut session = Session::new(test_project_path(), "short prompt".to_string());

        // Empty session should just have system prompt tokens
        let base_tokens = session.token_count();
        assert_eq!(base_tokens, "short prompt".len() / 4);

        // Add a text message
        session.add_user_message("hello world, this is a test message");
        let with_msg = session.token_count();
        assert!(with_msg > base_tokens);

        // Add assistant with blocks
        session.add_assistant_message(vec![
            ContentBlock::Text {
                text: "response text here".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "tool".to_string(),
                input: serde_json::json!({}),
            },
        ]);
        let with_blocks = session.token_count();
        // Non-text blocks count as 100 each
        assert!(with_blocks > with_msg);
    }

    #[test]
    fn test_compact() {
        let mut session = Session::new(test_project_path(), test_system_prompt());

        // Add 10 messages
        for i in 0..10 {
            session.add_user_message(&format!("message {}", i));
        }

        assert_eq!(session.messages.len(), 10);

        session.compact(1000);

        // After compaction, we should have: 1 summary message + 4 kept messages = 5
        assert_eq!(
            session.messages.len(),
            5,
            "should keep summary + last 4 messages"
        );

        // First message should be the summary
        let summary_text = session.messages[0].content.as_text().unwrap();
        assert!(
            summary_text.contains("earlier messages trimmed"),
            "summary should mention trimmed messages: {}",
            summary_text
        );

        // Last 4 should be the original last 4 messages
        assert_eq!(session.messages[1].content.as_text(), Some("message 6"));
        assert_eq!(session.messages[2].content.as_text(), Some("message 7"));
        assert_eq!(session.messages[3].content.as_text(), Some("message 8"));
        assert_eq!(session.messages[4].content.as_text(), Some("message 9"));
    }

    #[test]
    fn test_compact_few_messages() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("only message");

        let before = session.messages.len();
        session.compact(1000);

        // With only 1 message, keep=1, trimmed=0 -> no change
        assert_eq!(session.messages.len(), before);
        assert_eq!(session.messages[0].content.as_text(), Some("only message"));
    }

    #[test]
    fn test_info() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "hi".to_string(),
        }]);

        let info = session.info();
        assert_eq!(info.id, session.id);
        assert_eq!(info.title, session.title);
        assert_eq!(info.message_count, 2);
        assert_eq!(info.project_path, test_project_path());
        assert_eq!(info.created_at, session.created_at);
        assert_eq!(info.updated_at, session.updated_at);
    }

    #[test]
    fn test_set_system_prompt() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        let old_updated = session.updated_at;

        // Small delay so we can assert the timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(1));

        session.set_system_prompt("new prompt");

        assert_eq!(session.system_prompt, "new prompt");
        assert!(session.updated_at > old_updated);
    }

    #[test]
    fn test_revert_to() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        for i in 0..5 {
            session.add_user_message(&format!("msg {}", i));
        }

        assert_eq!(session.messages.len(), 5);

        // Keep messages up to and including index 2 (first 3 messages)
        let removed = session.revert_to(2);
        assert_eq!(removed, 2); // removed messages at indices 3 and 4
        assert_eq!(session.messages.len(), 3);
        assert_eq!(session.messages[0].content.as_text(), Some("msg 0"));
        assert_eq!(session.messages[1].content.as_text(), Some("msg 1"));
        assert_eq!(session.messages[2].content.as_text(), Some("msg 2"));
    }

    #[test]
    fn test_revert_to_out_of_bounds() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");

        let removed = session.revert_to(5); // beyond length
        assert_eq!(removed, 0);
        assert_eq!(session.messages.len(), 1);
    }

    #[test]
    fn test_conversation_text() {
        let mut session = Session::new(test_project_path(), test_system_prompt());
        session.add_user_message("hello");
        session.add_assistant_message(vec![ContentBlock::Text {
            text: "hi there".to_string(),
        }]);

        let text = session.conversation_text();
        assert!(text.contains("User: hello"));
        assert!(text.contains("Assistant: hi there"));
    }
}

/// Default system prompt for the main agent
pub fn default_system_prompt() -> String {
    include_str!("../../agent/prompt.txt").to_string()
}
