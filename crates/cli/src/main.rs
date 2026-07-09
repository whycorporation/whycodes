use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;

use whycode_agent::agent::Agent;
use whycode_core::config::Config;
use whycode_core::types::{AgentInfo, AgentMode, ModelConfig, PermissionSet};
use whycode_session::session::Session;

/// Whycode — An AI coding agent built in Rust
#[derive(Parser, Debug)]
#[command(name = "whycode", version, about = "AI-powered coding agent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Prompt to send directly to the agent (non-interactive mode)
    #[arg(short = 'p', long, group = "input")]
    pub prompt: Option<String>,

    /// Agent name to use (build, plan, explore, general)
    #[arg(short = 'a', long = "agent", default_value = "build")]
    pub agent: String,

    /// Project directory
    #[arg(short = 'd', long, default_value = ".")]
    pub dir: String,

    /// Provider to use
    #[arg(short = 'P', long, default_value = "anthropic")]
    pub provider: String,

    /// Model to use
    #[arg(short = 'm', long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Maximum conversation turns
    #[arg(short = 't', long, default_value = "25")]
    pub max_turns: usize,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start interactive mode
    Run,
    /// Show configuration
    Config,
    /// List available models
    Models,
    /// List available tools
    Tools,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let config = Config::load()?;

    match &cli.command {
        Some(Commands::Config) => {
            let config_path = Config::default_path()?;
            println!("Config path: {}", config_path.display());
            let text = toml::to_string_pretty(&config)?;
            println!("{}", text);
            Ok(())
        }
        Some(Commands::Models) => {
            println!("{}", "Configured models:".bold());
            for (_, model) in &config.models {
                println!("  {}/{}", model.provider_id, model.model_id);
            }
            if config.models.is_empty() {
                println!("  No models configured.");
            }
            Ok(())
        }
        Some(Commands::Tools) => {
            println!("{}", "Available tools:".bold());
            let tools = [
                "read     — Read a file",
                "write    — Write to a file",
                "edit     — Edit a file with find/replace",
                "grep     — Search code with regex",
                "glob     — Find files by pattern",
                "shell    — Run shell commands",
                "webfetch — Fetch web content",
                "websearch — Search the web",
            ];
            for tool in &tools {
                println!("  {}", tool);
            }
            Ok(())
        }
        Some(Commands::Run) | None => {
            let project_dir = if cli.dir == "." {
                std::env::current_dir()?
            } else {
                PathBuf::from(&cli.dir)
            };

            let api_key = get_api_key(&cli.provider, &config).unwrap_or_else(|| {
                eprintln!(
                    "{} {}",
                    "Error:".red().bold(),
                    format!(
                        "No API key for '{}'. Set {} env var.",
                        cli.provider,
                        provider_env_var(&cli.provider)
                    )
                );
                std::process::exit(1);
            });

            let agent_info = config.get_agent(&cli.agent).cloned().unwrap_or_else(|| {
                eprintln!(
                    "{} Agent '{}' not found in config, using build agent fallback.",
                    "Warning:".yellow().bold(),
                    cli.agent
                );
                AgentInfo {
                    name: "build".to_string(),
                    description: "Default build agent".to_string(),
                    mode: AgentMode::Primary,
                    permission: PermissionSet {
                        allowed_tools: None,
                        denied_tools: None,
                        allow_file_writes: true,
                        allow_network: true,
                        allow_shell: true,
                        allowed_paths: None,
                    },
                    model: Some(ModelConfig {
                        model_id: cli.model.clone(),
                        provider_id: cli.provider.clone(),
                        max_tokens: None,
                        temperature: None,
                        top_p: None,
                        thinking: None,
                        supports_tools: Some(true),
                        supports_images: None,
                    }),
                    system_prompt: None,
                    temperature: None,
                    top_p: None,
                }
            });

            // Resolve the system prompt: use the agent's explicit prompt, or fall back to the
            // prompt file for that agent name, or the default system prompt.
            let system_prompt = agent_info
                .system_prompt
                .clone()
                .unwrap_or_else(|| Agent::system_prompt_for(&cli.agent));

            let agent = Agent::new(agent_info);
            let mut session = Session::new(project_dir.clone(), system_prompt);

            println!(
                "{} {}",
                "Whycode".cyan().bold(),
                format!("[agent={}, provider={}, model={}]", cli.agent, cli.provider, cli.model).dimmed()
            );
            println!("{} {}", "Project:".dimmed(), project_dir.display().to_string().dimmed());
            println!();

            if let Some(prompt) = &cli.prompt {
                if prompt.is_empty() {
                    eprintln!("{}", "Error: empty prompt".red());
                    return Ok(());
                }
                session.add_user_message(prompt);
                match agent.run_turn(&mut session, &cli.provider, &cli.model, &api_key, cli.max_turns).await {
                    Ok(response) => {
                        if !response.is_empty() {
                            println!("\n{}", response);
                        }
                    }
                    Err(e) => {
                        eprintln!("{} {}", "Error:".red().bold(), e);
                        return Err(anyhow::anyhow!("{e}"));
                    }
                }
                return Ok(());
            }

            // Interactive mode
            println!("{}", "Interactive mode. Type /help for commands, /exit to quit.".dimmed());
            loop {
                use std::io::Write;
                let _ = std::io::stdout().flush();

                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_err() {
                    break;
                }
                let input = input.trim().to_string();
                if input.is_empty() { continue; }

                if input.starts_with('/') {
                    match input.as_str() {
                        "/exit" | "/quit" | "/q" => break,
                        "/help" | "/h" => {
                            println!("  /exit, /quit, /q — Exit");
                            println!("  /help, /h       — Help");
                            println!("  /clear          — Clear session");
                            println!("  /info           — Session info");
                            continue;
                        }
                        "/clear" => { session = Session::new(project_dir.clone(), agent.system_prompt()); println!("Cleared."); continue; }
                        "/info" => { let i = session.info(); println!("ID: {} | Messages: {} | {}", i.id, i.message_count, i.created_at.format("%H:%M:%S")); continue; }
                        cmd => { println!("Unknown: {}. /help for commands.", cmd); continue; }
                    }
                }

                session.add_user_message(&input);
                match agent.run_turn(&mut session, &cli.provider, &cli.model, &api_key, cli.max_turns).await {
                    Ok(response) => { if !response.is_empty() { println!("\n{}", response); } println!(); }
                    Err(e) => eprintln!("{} {}", "Error:".red().bold(), e),
                }
            }
            println!("{}", "Goodbye!".cyan());
            Ok(())
        }
    }
}

fn get_api_key(provider: &str, config: &Config) -> Option<String> {
    let env_var = provider_env_var(provider);
    if let Ok(key) = std::env::var(&env_var) {
        if !key.is_empty() { return Some(key); }
    }
    if let Some(pc) = config.get_provider(provider) {
        if let Some(key) = &pc.api_key { return Some(key.clone()); }
    }
    if provider == "openai" {
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            return Some(key);
        }
    }
    None
}

fn provider_env_var(provider: &str) -> &str {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "google" => "GOOGLE_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "xai" | "grok" => "XAI_API_KEY",
        _ => "API_KEY",
    }
}
