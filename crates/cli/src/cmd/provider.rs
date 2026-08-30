//! Provider, model, plugin, and agent commands.
use super::helpers::*;
use crate::Cli;
use crate::args::*;
use colored::*;
use whycodes_config::Config;
use whycodes_core::types::{ModelConfig, ProviderConfig};

pub(crate) async fn cmd_provider(cmd: &ProviderCmd) -> anyhow::Result<()> {
    let mut config = Config::load()?;

    match cmd {
        ProviderCmd::List => {
            if config.providers.is_empty() {
                println!("{} No providers configured.", "ℹ".cyan());
                println!();
                println!("Add a provider:");
                println!("  whycodes provider add <name> --api-key <key> --base-url <url>");
                println!();
                println!(
                    "Built-in providers supported: {}",
                    whycodes_llm::ProviderRegistry::default().names().join(", ")
                );
            } else {
                println!("{} Configured providers:", "🔑".bold());
                for (name, provider) in &config.providers {
                    let key_status = if provider.api_key.is_some() {
                        "✓".green()
                    } else {
                        "✗".red()
                    };
                    let url = provider
                        .base_url
                        .as_deref()
                        .or(provider.api_base.as_deref())
                        .unwrap_or("(default)");
                    println!(
                        "  {} {}  API key: {}  Base URL: {}",
                        name.cyan(),
                        key_status,
                        if provider.api_key.is_some() {
                            "set"
                        } else {
                            "not set"
                        },
                        url
                    );
                }
            }
        }
        ProviderCmd::Add {
            name,
            api_key,
            base_url,
            headers,
        } => {
            let mut headers_map = std::collections::HashMap::new();
            if let Some(h) = headers {
                for pair in h.split(',') {
                    let parts: Vec<&str> = pair.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        headers_map
                            .insert(parts[0].trim().to_string(), parts[1].trim().to_string());
                    }
                }
            }

            let provider = ProviderConfig {
                name: name.clone(),
                api_key: api_key.clone(),
                api_base: None,
                base_url: base_url.clone(),
                headers: if headers_map.is_empty() {
                    None
                } else {
                    Some(headers_map)
                },
                models: Vec::new(),
                tool_arguments: None,
                extra: std::collections::HashMap::new(),
            };

            if config.providers.contains_key(name) {
                println!(
                    "{} Provider '{}' already exists. Updating...",
                    "⚠".yellow(),
                    name.cyan()
                );
            }

            config.providers.insert(name.clone(), provider);
            config.save()?;
            println!(
                "{} Provider '{}' {}",
                "✓".green(),
                name.cyan(),
                if api_key.is_some() {
                    "added with API key"
                } else {
                    "added (no API key — set via env var or --api-key)"
                }
            );
        }
        ProviderCmd::Remove { name } => {
            if config.providers.remove(name).is_some() {
                config.save()?;
                println!("{} Provider '{}' removed.", "✓".green(), name.cyan());
            } else {
                eprintln!("{} Provider '{}' not found.", "✗".red(), name.cyan());
            }
        }
        ProviderCmd::Default { name } => {
            if config.providers.contains_key(name) {
                // Save provider name as metadata
                config.save()?;
                println!("{} Default provider set to '{}'.", "✓".green(), name.cyan());
                println!("  Use: whycodes -P {} ...", name);
            } else {
                eprintln!(
                    "{} Provider '{}' not found. Add it first: whycodes provider add {}",
                    "✗".red(),
                    name.cyan(),
                    name
                );
            }
        }
    }

    Ok(())
}

/// `model` — Model management
pub(crate) async fn cmd_model(cmd: &ModelCmd) -> anyhow::Result<()> {
    let config = Config::load()?;

    match cmd {
        ModelCmd::List => {
            if config.models.is_empty() {
                println!("{} No models configured.", "ℹ".cyan());
                println!();
                println!("Configure models in your config file:");
                if let Ok(path) = Config::default_path() {
                    println!("  {}", path.display());
                }
            } else {
                println!("{} Configured models:", "🤖".bold());
                for (key, model) in &config.models {
                    println!(
                        "  {} → {}/{}",
                        key.cyan(),
                        model.provider_id.dimmed(),
                        model.model_id
                    );
                    if let Some(max_tok) = model.max_tokens {
                        println!("    max_tokens: {}", max_tok);
                    }
                }
            }
        }
        ModelCmd::Default { provider, model } => {
            let mut config = config.clone();
            config.default_model = Some(ModelConfig {
                model_id: model.clone(),
                provider_id: provider.clone(),
                max_tokens: None,
                context_window: None,
                temperature: None,
                top_p: None,
                thinking: None,
                supports_tools: Some(true),
                supports_images: None,
            });
            config.save()?;
            println!(
                "{} Default model set to {}/{}",
                "✓".green(),
                provider.cyan(),
                model.cyan()
            );
        }
    }

    Ok(())
}

/// `agent` — Agent configuration
pub(crate) async fn cmd_plugins(cli: &Cli, cmd: Option<&PluginsCmd>) -> anyhow::Result<()> {
    let _ = cmd; // only List for now
    let project = resolve_dir(cli);
    let listed = whycodes_tools::list_shell_plugins(Some(&project));
    if listed.is_empty() {
        println!("{} No shell plugins configured.", "🔌".bold());
        println!("TOML: ~/.config/whycodes/plugins.toml or");
        println!("      .whycodes/plugins.toml");
        println!();
        println!("  [[plugins]]");
        println!("  name = \"hello\"");
        println!("  command = \"echo hello from plugin\"");
        println!("  description = \"Demo plugin\"");
        println!();
        println!("Or a directory plugin:");
        println!("  .whycodes/plugins/hello/plugin.json");
        println!("  {{\"name\":\"hello\",\"command\":\"./run.sh\",\"description\":\"Demo\"}}");
        println!();
        println!("Tools appear as plugin_<name> (tool_profile=full or tool_search).");
        return Ok(());
    }
    println!("{} Shell plugins ({}):", "🔌".bold(), listed.len());
    for p in &listed {
        println!(
            "  {} → {} — {} ({})",
            p.tool_name.cyan(),
            p.command.dimmed(),
            p.description,
            p.origin.dimmed()
        );
    }
    Ok(())
}

pub(crate) async fn cmd_agent(name: Option<&str>) -> anyhow::Result<()> {
    let config = Config::load()?;

    match name {
        Some(name) => {
            if let Some(agent) = config.get_agent(name) {
                println!("{} Agent: {}", "🤖".bold(), agent.name.cyan().bold());
                println!("  Description: {}", agent.description);
                println!("  Mode: {:?}", agent.mode);
                println!("  File writes: {}", agent.permission.allow_file_writes);
                println!("  Network:     {}", agent.permission.allow_network);
                println!("  Shell:       {}", agent.permission.allow_shell);
                if let Some(ref tools) = agent.permission.allowed_tools {
                    println!("  Allowed tools: {:?}", tools);
                }
                if let Some(ref tools) = agent.permission.denied_tools {
                    println!("  Denied tools: {:?}", tools);
                }
                if let Some(ref model) = agent.model {
                    println!("  Model: {}/{}", model.provider_id, model.model_id);
                }
            } else {
                eprintln!("{} Agent '{}' not found.", "✗".red(), name);
                println!();
                println!("Available agents:");
                for a in &config.agents {
                    println!("  {} — {}", a.name.cyan(), a.description);
                }
            }
        }
        None => {
            println!("{} Available agents:", "🤖".bold());
            for agent in &config.agents {
                let default_marker = if agent.name == config.default_agent {
                    " (default)".dimmed()
                } else {
                    "".into()
                };
                println!(
                    "  {}{} — {}",
                    agent.name.cyan(),
                    default_marker,
                    agent.description
                );
            }
            if config.agents.is_empty() {
                println!("  (no agents configured)");
            }
        }
    }

    Ok(())
}
