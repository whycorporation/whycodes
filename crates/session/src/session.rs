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
        thinking: Option<bool>,
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

        Ok(share_path.to_string_lossy().to_string())
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

    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now();
    }
}

/// Default system prompt for the main agent
pub fn default_system_prompt() -> String {
    include_str!("../../agent/prompt.txt").to_string()
}
