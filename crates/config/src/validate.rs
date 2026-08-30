//! Config validation.

use crate::types::{Config, NotifyEvent, is_discord_webhook_url};
use whycodes_core::{Error, Result};

impl Config {
    /// Validate the configuration and return any issues.
    ///
    /// Checks required fields and emits warnings for common misconfigurations.
    pub fn validate(&self) -> Result<()> {
        let mut issues: Vec<String> = Vec::new();

        // Check that at least one provider is configured or default model exists
        if self.providers.is_empty() && self.default_model.is_none() {
            issues.push(
                "No providers configured and no default model set. \
                 Configure at least one provider or set WHYCODES_PROVIDER / WHYCODES_MODEL."
                    .to_string(),
            );
        }

        // Check default_model has a provider_id if set
        if let Some(ref dm) = self.default_model {
            if dm.provider_id.is_empty() && !self.providers.is_empty() {
                issues.push(format!(
                    "Default model '{}' has no provider_id but {} provider(s) are configured. \
                     Specify a provider_id for the default model.",
                    dm.model_id,
                    self.providers.len()
                ));
            }
            if dm.model_id.is_empty() {
                issues.push("Default model has an empty model_id.".to_string());
            }
        }

        // Check providers for common issues
        for (name, provider) in &self.providers {
            if provider.api_key.is_none() {
                // Check env var: <NAME>_API_KEY or WHYCODES_<NAME>_API_KEY
                let key_from_env = std::env::var(format!("{}_API_KEY", name.to_uppercase()))
                    .or_else(|_| std::env::var(format!("WHYCODES_{}_API_KEY", name.to_uppercase())))
                    .or_else(|_| std::env::var("OPENAI_API_KEY"))
                    .or_else(|_| std::env::var("ANTHROPIC_API_KEY"));

                if key_from_env.is_err() {
                    issues.push(format!(
                        "Provider '{}' has no api_key configured and no matching \
                         environment variable found ({}_API_KEY, OPENAI_API_KEY, or ANTHROPIC_API_KEY).",
                        name,
                        name.to_uppercase()
                    ));
                }
            }

            // Warn if base_url is set to a known developer/local endpoint
            if let Some(ref url) = provider.base_url
                && (url.contains("localhost") || url.contains("127.0.0.1"))
            {
                issues.push(format!(
                    "Provider '{}' base_url points to localhost ({}). \
                     This is fine for local development but will not work in production.",
                    name, url
                ));
            }
        }

        if let Some(url) = self.notify.discord_webhook.as_deref()
            && !url.trim().is_empty()
            && !is_discord_webhook_url(url)
        {
            issues.push(
                "notify.discord_webhook is set but is not a Discord Incoming Webhook URL \
                 (https://discord.com/api/webhooks/… or https://discordapp.com/api/webhooks/…)."
                    .to_string(),
            );
        }
        if self
            .notify
            .telegram_bot_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
            != self
                .notify
                .telegram_chat_id
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty())
        {
            issues.push(
                "Telegram notify needs both notify.telegram_bot_token and notify.telegram_chat_id \
                 (or WHYCODES_NOTIFY_TELEGRAM_BOT_TOKEN / WHYCODES_NOTIFY_TELEGRAM_CHAT_ID)."
                    .to_string(),
            );
        }
        if self.notify.timeout_secs == 0 || self.notify.timeout_secs > 60 {
            issues.push(format!(
                "notify.timeout_secs is {} (expected 1–60).",
                self.notify.timeout_secs
            ));
        }
        for event in &self.notify.on {
            if NotifyEvent::parse(event).is_none() {
                issues.push(format!(
                    "notify.on contains unknown event '{event}'. Expected turn_done and/or need_input."
                ));
            }
        }

        // Check session config
        if self.session.max_context_tokens == 0 {
            issues.push(
                "session.max_context_tokens is set to 0. This will disable context.".to_string(),
            );
        }
        if self.session.race_after_ms > 30_000 {
            issues.push(format!(
                "session.race_after_ms is {} (>30s). First-token race will wait a long time.",
                self.session.race_after_ms
            ));
        }
        let rc = self.session.response_cache.trim().to_ascii_lowercase();
        if !matches!(
            rc.as_str(),
            "auto" | "on" | "true" | "1" | "off" | "false" | "0" | "none"
        ) {
            issues.push(format!(
                "session.response_cache is '{}'; expected auto or off.",
                self.session.response_cache
            ));
        }

        // Check agents
        if self.agents.is_empty() {
            issues.push(
                "No agents configured. At least one agent is recommended for proper operation."
                    .to_string(),
            );
        }

        // Check that default_agent resolves to an existing agent
        if !self.default_agent.is_empty()
            && !self.agents.iter().any(|a| a.name == self.default_agent)
        {
            issues.push(format!(
                "Default agent '{}' not found in the agents list.",
                self.default_agent
            ));
        }

        // Report issues
        if issues.is_empty() {
            tracing::info!("Configuration validated successfully.");
            Ok(())
        } else {
            for issue in &issues {
                if issue.contains("localhost") || issue.contains("127.0.0.1") {
                    tracing::warn!("{}", issue);
                } else {
                    tracing::warn!("Config issue: {}", issue);
                }
            }
            // Return the first real error if any; otherwise it's just warnings
            let errors: Vec<&String> = issues
                .iter()
                .filter(|i| !i.contains("localhost") && !i.contains("127.0.0.1"))
                .collect();

            if errors.is_empty() {
                // Only localhost warnings — still ok
                Ok(())
            } else if errors.len() == 1 {
                Err(Error::Config(errors[0].clone()))
            } else {
                Err(Error::Config(
                    errors
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                ))
            }
        }
    }
}
