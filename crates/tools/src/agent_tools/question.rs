use async_trait::async_trait;
use serde_json::json;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::tool::{Tool, ToolContext};
use whycode_core::types::ToolResult;

pub struct QuestionTool;

impl Default for QuestionTool {
    fn default() -> Self {
        Self::new()
    }
}

impl QuestionTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        "Ask the user a question when clarification is needed"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "choices": {
                    "type": "array",
                    "description": "Optional list of predefined choices for the user",
                    "items": {
                        "type": "string"
                    }
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let question = args["question"].as_str().unwrap_or("(no question)");
        let choices: Option<Vec<String>> =
            args.get("choices").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        // Print the question to stderr
        let mut prompt = format!("\n❓ {}\n", question);
        if let Some(ref choices) = choices {
            prompt.push_str("\nChoices:\n");
            for (i, choice) in choices.iter().enumerate() {
                prompt.push_str(&format!("  {}. {}\n", i + 1, choice));
            }
            prompt.push_str("\nEnter the number of your choice (or type your answer): ");
        } else {
            prompt.push_str("\nEnter your answer: ");
        }

        eprint!("{}", prompt);
        let _ = io::stderr().flush();

        // Read stdin with a timeout
        let done = Arc::new(AtomicBool::new(false));
        let done_clone = Arc::clone(&done);

        // Spawn a thread to listen for input
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            match io::stdin().read(&mut buf) {
                Ok(n) if n > 0 => {
                    let s = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                    done_clone.store(true, Ordering::SeqCst);
                    Some(s)
                }
                _ => {
                    done_clone.store(true, Ordering::SeqCst);
                    None
                }
            }
        });

        // Wait up to 60 seconds for a response
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(60);

        loop {
            if done.load(Ordering::SeqCst) {
                break;
            }
            if start.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // If we timed out, the thread may still be blocked on stdin read.
        // We can't cleanly kill it in safe Rust; just report the timeout.
        let answer = if done.load(Ordering::SeqCst) {
            match handle.join() {
                Ok(Some(s)) => s,
                _ => String::new(),
            }
        } else {
            eprintln!("\n⏰ Timed out waiting for user input.");
            String::new()
        };

        let result_content = if answer.is_empty() {
            "No answer received (timeout or empty input).".to_string()
        } else {
            // If choices were provided, try to resolve numeric choice
            if let Some(ref choices) = choices
                && let Ok(num) = answer.parse::<usize>()
                && num >= 1
                && num <= choices.len()
            {
                let chosen = &choices[num - 1];
                return ToolResult {
                    tool_call_id: String::new(),
                    content: format!(
                        "Question: {}\nAnswer (choice #{}): {}",
                        question, num, chosen
                    ),
                    is_error: false,
                };
            }
            format!("Question: {}\nAnswer: {}", question, answer)
        };

        ToolResult {
            tool_call_id: String::new(),
            content: result_content,
            is_error: answer.is_empty(),
        }
    }
}
