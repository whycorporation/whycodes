//! Decision models as a selectable provider (`decision/<model>`).
//!
//! A decision model answers a typed question with probabilities instead of
//! text. Picking one in `/models` turns the prompt into a question box: the
//! last user message becomes one question, sent to a `/systemone` endpoint
//! (Ollama, llama.cpp, OpenRouter, TypeSafe), and the answer comes back as a
//! short text reply with probability bars. No tools, no other context.
//!
//! Prompt shape:
//!
//! ```text
//! Is this PR ready to merge?          → yes/no (`noul`)
//!
//! Which database should we use?       → choice
//! - postgres
//! - sqlite
//! ```

use futures::stream;
use serde_json::{Value, json};
use whycodes_core::types::{ContentBlock, LlmRequest, LlmResponse, Role, StreamEvent, Usage};

use crate::provider::{
    LlmProvider, ProviderEventStream, ProviderResponseFuture, ProviderStreamFuture,
};

/// Provider id used in the model picker and session routes.
pub const DECISION_PROVIDER: &str = "decision";

const OLLAMA_BASE: &str = "http://127.0.0.1:11434/v1";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// A decision model and where it is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionModel {
    /// Model id shown in the picker and sent to the endpoint.
    pub id: &'static str,
    pub base_url: &'static str,
    /// Environment variable with the bearer token; empty for a local server.
    pub key_env: &'static str,
}

const fn openrouter(id: &'static str) -> DecisionModel {
    DecisionModel {
        id,
        base_url: OPENROUTER_BASE,
        key_env: OPENROUTER_KEY_ENV,
    }
}

/// Decision models listed in the picker: local Ollama first, then OpenRouter.
pub const DECISION_MODELS: &[DecisionModel] = &[
    DecisionModel {
        id: "clef-flash",
        base_url: OLLAMA_BASE,
        key_env: "",
    },
    openrouter("cloudflare/clef"),
    openrouter("perplexity/pplx-decider-v1.2"),
    openrouter("liquid/d1"),
    openrouter("microsoft/microsoft-decision-1"),
    openrouter("upstage/solar-decide-flash"),
    openrouter("cloudflare/clef-omni"),
    openrouter("google/gemini-decision-1"),
    openrouter("perplexity/pplx-decider-flash-v1.2"),
    openrouter("typesafe/jev-1.14"),
    openrouter("openai/gpt-6-luna-decisions"),
    openrouter("cloudflare/clef-flash"),
];

/// One typed question parsed from a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionQuestion {
    /// The prompt without its option lines.
    pub question: String,
    /// `- option` lines; two or more make a `choice` question.
    pub options: Vec<String>,
}

impl DecisionQuestion {
    pub fn parse(prompt: &str) -> Self {
        let mut question = Vec::new();
        let mut options = Vec::new();
        for line in prompt.lines() {
            let trimmed = line.trim_start();
            match trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
            {
                Some(option) if !option.trim().is_empty() => {
                    options.push(option.trim().to_string())
                }
                _ => question.push(line),
            }
        }
        if options.len() < 2 {
            // A single bullet is part of the question, not a choice.
            return Self {
                question: prompt.trim().to_string(),
                options: Vec::new(),
            };
        }
        Self {
            question: question.join("\n").trim().to_string(),
            options,
        }
    }

    /// `/systemone` request body for `model`.
    pub fn body(&self, model: &str) -> Value {
        let question = if self.options.is_empty() {
            json!({"type": "noul", "instructions": self.question})
        } else {
            let criteria: serde_json::Map<String, Value> =
                self.options.iter().map(|o| (o.clone(), json!(o))).collect();
            json!({"type": "choice", "instructions": self.question, "criteria": criteria})
        };
        json!({"model": model, "state": self.question, "questions": {"answer": question}})
    }
}

/// Text reply for a `/systemone` response: options by probability with bars.
/// `Err` for a refusal or any other shape.
pub fn format_answer(json: &Value) -> Result<String, String> {
    let answer = &json["answers"]["answer"];
    let mut rows: Vec<(String, f64)> = match answer["type"].as_str() {
        Some("noul") => {
            let yes = answer["noul"]
                .as_f64()
                .ok_or("noul answer without a value")?;
            vec![("yes".into(), yes), ("no".into(), 1.0 - yes)]
        }
        Some("choice") => answer["probabilities"]
            .as_object()
            .ok_or("choice answer without probabilities")?
            .iter()
            .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
            .collect(),
        Some("refusal") => return Err("the model declined to answer".into()),
        _ => return Err("no answer in the response".into()),
    };
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let width = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    // Headline in bold, then aligned bars in a code block (monospace).
    let mut out = format!(
        "**{}** · {:.0}%

```text
",
        rows[0].0,
        rows[0].1 * 100.0
    );
    for (label, p) in &rows {
        let filled = (p.clamp(0.0, 1.0) * 10.0).round() as usize;
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
        out.push_str(&format!(
            "{label:<width$}  {bar} {:>3.0}%
",
            p * 100.0
        ));
    }
    out.push_str(
        "```
",
    );
    if let Some(c) = answer["confidence"].as_f64() {
        out.push_str(&format!(
            "confidence {:.0}%
",
            c * 100.0
        ));
    }
    Ok(out)
}

/// The decision provider.
#[derive(Debug, Clone)]
pub struct DecisionProvider {
    /// `[providers.openrouter] api_key`, used when `OPENROUTER_API_KEY` is unset.
    openrouter_key: Option<String>,
    /// `/v1` base of the local server for local models (`[providers.ollama]`).
    local_base: String,
}

impl Default for DecisionProvider {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl DecisionProvider {
    /// `local_base` is an Ollama-style host (`http://host:11434`, with or
    /// without `/v1`); `None` means `127.0.0.1:11434`.
    pub fn new(openrouter_key: Option<String>, local_base: Option<&str>) -> Self {
        let local_base = match local_base.map(|b| b.trim().trim_end_matches('/')) {
            Some(b) if b.ends_with("/v1") => b.to_string(),
            Some(b) if !b.is_empty() => format!("{b}/v1"),
            _ => OLLAMA_BASE.to_string(),
        };
        Self {
            openrouter_key: openrouter_key.filter(|k| !k.is_empty()),
            local_base,
        }
    }

    pub fn from_config(config: &whycodes_config::Config) -> Self {
        let ollama = config.get_provider("ollama");
        Self::new(
            config
                .get_provider("openrouter")
                .and_then(|p| p.api_key.clone()),
            ollama.and_then(|p| p.base_url.as_deref().or(p.api_base.as_deref())),
        )
    }

    /// Endpoint and bearer token for `model`. A model id not in the list is
    /// sent to OpenRouter.
    fn endpoint(&self, model: &str, api_key: &str) -> (String, Option<String>) {
        let entry = DECISION_MODELS
            .iter()
            .find(|m| m.id == model)
            .copied()
            .unwrap_or_else(|| openrouter(""));
        let key = match entry.key_env {
            "" => None,
            env => std::env::var(env)
                .ok()
                .or_else(|| (!api_key.is_empty()).then(|| api_key.to_string()))
                .or_else(|| self.openrouter_key.clone())
                .filter(|k| !k.is_empty()),
        };
        let base = match entry.base_url {
            OLLAMA_BASE => self.local_base.as_str(),
            other => other,
        };
        (format!("{base}/systemone"), key)
    }

    /// Ask the question in `request` and return the reply text and usage.
    async fn answer(
        &self,
        request: &LlmRequest,
        api_key: &str,
        model: &str,
    ) -> whycodes_core::Result<(String, Usage)> {
        let prompt = last_user_text(request)
            .ok_or_else(|| whycodes_core::Error::llm("no question to ask the decision model"))?;
        let question = DecisionQuestion::parse(&prompt);
        let (url, key) = self.endpoint(model, api_key);
        let mut call = crate::client_identity::http_client()
            .post(&url)
            .json(&question.body(model));
        if let Some(key) = key {
            call = call.bearer_auth(key);
        }
        let response = call
            .send()
            .await
            .map_err(|e| whycodes_core::Error::llm(format!("decision model {model}: {e}")))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let detail = body["error"]["message"]
                .as_str()
                .or_else(|| body["error"].as_str())
                .or_else(|| body["detail"]["message"].as_str())
                .unwrap_or("");
            return Err(whycodes_core::Error::Provider(format!(
                "decision model {model}: HTTP {status} {detail}"
            )));
        }
        let text = format_answer(&body)
            .map_err(|e| whycodes_core::Error::Provider(format!("decision model {model}: {e}")))?;
        let usage = Usage {
            input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0),
            ..Usage::default()
        };
        Ok((text, usage))
    }
}

/// Text of the last user message (the question).
fn last_user_text(request: &LlmRequest) -> Option<String> {
    request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .and_then(|m| m.content.as_text())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
}

impl LlmProvider for DecisionProvider {
    fn name(&self) -> &str {
        DECISION_PROVIDER
    }

    fn default_base_url(&self) -> &str {
        OPENROUTER_BASE
    }

    /// The prompt is the question; recall, posture, titles and summaries
    /// are not decision questions.
    fn prompt_only(&self) -> bool {
        true
    }

    fn complete<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderResponseFuture<'a> {
        Box::pin(async move {
            let (text, usage) = self.answer(request, api_key, model).await?;
            Ok(LlmResponse {
                content: vec![ContentBlock::Text { text }],
                stop_reason: Some("end_turn".into()),
                usage,
                model: model.to_string(),
            })
        })
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
        api_key: &'a str,
        model: &'a str,
    ) -> ProviderStreamFuture<'a> {
        Box::pin(async move {
            let (text, usage) = self.answer(request, api_key, model).await?;
            let events = vec![
                Ok(StreamEvent::TextDelta { text }),
                Ok(StreamEvent::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                }),
                Ok(StreamEvent::MessageStop),
            ];
            Ok(Box::pin(stream::iter(events)) as ProviderEventStream)
        })
    }
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
