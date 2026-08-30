//! CLI command bodies. `Commands` stays in `args.rs`; this module owns dispatch.
use crate::{Cli, Commands};

pub(crate) mod auth;
pub(crate) mod config;
pub(crate) mod debug;
mod github;
mod helpers;
mod mcp;
mod memory;
mod provider;
mod run;
mod serve;
mod session;
#[cfg(feature = "self-update")]
mod upgrade;

#[allow(unused_imports)] // re-exported for `main` tests via `use cmd::*`
pub(crate) use auth::*;
#[allow(unused_imports)]
pub(crate) use config::*;
#[allow(unused_imports)]
pub(crate) use debug::*;
#[allow(unused_imports)]
pub(crate) use github::*;
pub(crate) use helpers::*;
#[allow(unused_imports)]
pub(crate) use mcp::*;
#[allow(unused_imports)]
pub(crate) use memory::*;
#[allow(unused_imports)]
pub(crate) use provider::*;
#[allow(unused_imports)]
pub(crate) use run::*;
#[allow(unused_imports)]
pub(crate) use serve::*;
#[allow(unused_imports)]
pub(crate) use session::*;
#[cfg(feature = "self-update")]
#[allow(unused_imports)]
pub(crate) use upgrade::*;

pub(crate) async fn dispatch_command(cmd: &Commands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        Commands::Run {
            prompt,
            max_turns,
            format,
        } => run::cmd_run(cli, prompt.as_deref(), *max_turns, *format).await,
        Commands::Generate {
            prompt,
            max_turns,
            jobs,
            format,
        } => run::cmd_generate(cli, prompt, *max_turns, *jobs, *format).await,
        Commands::Acp => github::cmd_acp(cli).await,
        Commands::Pr { title, base } => {
            github::cmd_pr(cli, title.as_deref(), base.as_deref()).await
        }
        Commands::Github { cmd: gh_cmd } => github::cmd_github(cli, gh_cmd).await,
        #[cfg(feature = "server")]
        Commands::Serve { port } => serve::cmd_serve(*port).await,
        Commands::Connect { addr, session } => {
            serve::cmd_connect(cli, addr, session.as_deref()).await
        }
        Commands::Web => serve::cmd_web().await,
        Commands::Mcp { cmd: mcp_cmd } => mcp::cmd_mcp(mcp_cmd).await,
        Commands::Provider { cmd: provider_cmd } => provider::cmd_provider(provider_cmd).await,
        Commands::Model { cmd: model_cmd } => provider::cmd_model(model_cmd).await,
        Commands::Agent { name } => provider::cmd_agent(name.as_deref()).await,
        Commands::Plugins { cmd } => provider::cmd_plugins(cli, cmd.as_ref()).await,
        Commands::Config { cmd: config_cmd } => config::cmd_config(config_cmd).await,
        Commands::Session { cmd: session_cmd } => session::cmd_session(session_cmd).await,
        Commands::Memory { cmd: memory_cmd } => memory::cmd_memory(cli, memory_cmd).await,
        Commands::Auth { cmd } => auth::cmd_auth(cmd).await,
        Commands::Stats => session::cmd_stats().await,
        Commands::Debug => debug::cmd_debug().await,
        #[cfg(feature = "self-update")]
        Commands::Upgrade => upgrade::cmd_upgrade().await,
        Commands::Completions { shell } => cmd_completions(*shell),
    }
}
