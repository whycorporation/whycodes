//! Decision-model client for the shell second opinion (`[security.decision]`).
//!
//! A decision model answers typed questions about a state with probabilities
//! instead of text. One request asks two questions about a shell command:
//! how much it could destroy (`choice`) and whether it fits what the user
//! asked for (`noul`). The answers fold into one score in `0.0..=2.0`:
//!
//! ```text
//! risk = p(project_unrecoverable) + p(outside_project) + p(remote_or_irreversible)
//!      + (1 - p(fits the request))
//! ```
//!
//! The wire shape is `/systemone` (TypeSafe's API), which Ollama, llama.cpp,
//! OpenRouter and TypeSafe serve. Every failure is an `Err`; the caller keeps
//! the decision it already had. See `docs/proposals/decision-models.md`.

use std::time::Duration;

use serde_json::{Value, json};
use whycodes_config::DecisionConfig;

const BLAST_INSTRUCTIONS: &str = "A coding agent wants to run this shell command in the user's project. What is the worst realistic effect if it runs now?";
const FITS_INSTRUCTIONS: &str =
    "Is running this command a reasonable, expected step toward what the user asked for?";

/// The `choice` options, in the order they are sent. The last three count
/// toward the risk score.
const BLAST_OPTIONS: [(&str, &str); 5] = [
    ("read_only", "Only reads files or prints information"),
    (
        "project_recoverable",
        "Changes files inside the project that git or a rebuild can restore",
    ),
    (
        "project_unrecoverable",
        "Deletes or overwrites project data that cannot be restored",
    ),
    (
        "outside_project",
        "Changes files, programs, processes or settings outside the project",
    ),
    (
        "remote_or_irreversible",
        "Publishes, deploys, sends data off the machine, or deletes remote data",
    ),
];

/// What the model said about one command.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellVerdict {
    /// Combined score, `0.0..=2.0`.
    pub risk: f64,
    /// Most likely `choice` option (`outside_project`, …).
    pub effect: String,
    /// Its probability.
    pub effect_probability: f64,
    /// Probability that the command fits the user's request.
    pub fits: f64,
}

impl ShellVerdict {
    /// One line for the permission dialog, e.g.
    /// `decision model (clef-flash) · outside project (0.71) · does not match your request`.
    pub fn reason(&self, model: &str) -> String {
        let mut reason = format!(
            "decision model ({model}) · {} ({:.2})",
            self.effect.replace('_', " "),
            self.effect_probability
        );
        if self.fits < 0.5 {
            reason.push_str(" · does not match your request");
        }
        reason
    }
}

/// A decision endpoint, built from `[security.decision]`.
#[derive(Debug, Clone)]
pub struct DecisionClient {
    url: String,
    model: String,
    api_key: Option<String>,
    timeout: Duration,
    provider_only: Vec<String>,
}

impl DecisionClient {
    /// `api_key` is the resolved bearer token (from `api_key_env`), if any.
    pub fn new(config: &DecisionConfig, api_key: Option<String>) -> Self {
        Self {
            url: format!("{}/systemone", config.base_url.trim_end_matches('/')),
            model: config.model.clone(),
            api_key: api_key.filter(|k| !k.is_empty()),
            timeout: Duration::from_millis(config.timeout_ms),
            provider_only: config.provider_only.clone(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// The request body for `command` asked on behalf of `user_request`.
    /// Both are masked with [`mask_for_decision`] and capped in length; the
    /// caller is expected to have run the secrets redactor first.
    pub fn body(&self, command: &str, user_request: &str) -> Value {
        let command = mask_for_decision(command, MAX_COMMAND_CHARS);
        let user_request = mask_for_decision(user_request, MAX_REQUEST_CHARS);
        let criteria: serde_json::Map<String, Value> = BLAST_OPTIONS
            .iter()
            .map(|(k, v)| ((*k).to_string(), json!(v)))
            .collect();
        let mut body = json!({
            "model": self.model,
            "state": {
                "command": command,
                "working_dir": "project root",
                "user_request": user_request,
            },
            "questions": {
                "blast": {"type": "choice", "instructions": BLAST_INSTRUCTIONS, "criteria": criteria},
                "fits": {"type": "noul", "instructions": FITS_INSTRUCTIONS},
            },
        });
        if !self.provider_only.is_empty() {
            body["provider"] = json!({"only": self.provider_only});
        }
        body
    }

    /// Ask about `command`. `Err` carries a short reason for the log.
    pub async fn shell_verdict(
        &self,
        command: &str,
        user_request: &str,
    ) -> Result<ShellVerdict, String> {
        let mut request = crate::client_identity::http_client()
            .post(&self.url)
            .timeout(self.timeout)
            .json(&self.body(command, user_request));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP {status}"));
        }
        let json: Value = response.json().await.map_err(|e| e.to_string())?;
        parse_verdict(&json)
    }
}

const MAX_COMMAND_CHARS: usize = 4000;
const MAX_REQUEST_CHARS: usize = 2000;

/// Mask what the secrets redactor's token shapes miss but a pasted URL often
/// carries (login payloads, signed links): query-string values of 24+
/// characters and long runs of percent-encoding. Then cap at `max` chars.
pub fn mask_for_decision(text: &str, max: usize) -> String {
    // Group 1 is kept; the rest of the match becomes `<redacted>`.
    static MASKS: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
        crate::secrets::compile_patterns(&[
            r"([?&][A-Za-z0-9_.\-]+=)[^&\s'\x22]{24,}",
            r"()(?:%[0-9A-Fa-f]{2}[^%\s&]{0,12}){6,}",
        ])
    });
    let mut masked = text.to_string();
    for re in MASKS.iter() {
        masked = re.replace_all(&masked, "${1}<redacted>").into_owned();
    }
    masked.chars().take(max).collect()
}

/// Fold a `/systemone` response into a [`ShellVerdict`]. A `refusal` answer or
/// any other shape is an `Err`.
pub fn parse_verdict(json: &Value) -> Result<ShellVerdict, String> {
    let answers = &json["answers"];
    let probabilities = answers["blast"]["probabilities"]
        .as_object()
        .filter(|_| answers["blast"]["type"] == "choice")
        .ok_or("no blast choice in response")?;
    let fits = answers["fits"]["noul"]
        .as_f64()
        .filter(|_| answers["fits"]["type"] == "noul")
        .ok_or("no fits answer in response")?;
    let p = |key: &str| {
        probabilities
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let (effect, effect_probability) =
        BLAST_OPTIONS
            .iter()
            .map(|(k, _)| (*k, p(k)))
            .fold(("read_only", f64::MIN), |best, cur| {
                if cur.1 > best.1 { cur } else { best }
            });
    Ok(ShellVerdict {
        risk: p("project_unrecoverable")
            + p("outside_project")
            + p("remote_or_irreversible")
            + (1.0 - fits),
        effect: effect.to_string(),
        effect_probability,
        fits,
    })
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
