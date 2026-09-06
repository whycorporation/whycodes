//! MCP subcommands.
use crate::args::*;
use colored::*;
use whycodes_config::Config;

pub(crate) fn parse_mcp_transport(
    raw: Option<&str>,
) -> anyhow::Result<Option<whycodes_config::McpTransportKind>> {
    match raw {
        None => Ok(None),
        Some("stdio") | Some("local") => Ok(Some(whycodes_config::McpTransportKind::Stdio)),
        Some("http") | Some("streamable-http") | Some("remote") => {
            Ok(Some(whycodes_config::McpTransportKind::Http))
        }
        Some("sse") => Ok(Some(whycodes_config::McpTransportKind::Sse)),
        Some("auto") => Ok(Some(whycodes_config::McpTransportKind::Auto)),
        Some(other) => {
            anyhow::bail!("unknown MCP transport '{other}' (expected stdio|http|sse|auto)")
        }
    }
}

pub(crate) fn parse_mcp_headers(
    headers: &[String],
) -> anyhow::Result<Option<std::collections::HashMap<String, String>>> {
    if headers.is_empty() {
        return Ok(None);
    }
    let mut map = std::collections::HashMap::new();
    for h in headers {
        let (k, v) = h
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid --header '{h}' (expected 'Key: Value')"))?;
        map.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(Some(map))
}

pub(crate) fn mcp_remote_line(name: &str, kind: &str, url: &str) -> String {
    format!("  {} → {} {url}", name.cyan(), kind.dimmed())
}

pub(crate) fn mcp_stdio_line(name: &str, command: &str, args: &[String]) -> String {
    let args = if args.is_empty() {
        String::new()
    } else {
        format!(" {}", args.join(" "))
    };
    format!(
        "  {} → {}{}{}",
        name.cyan(),
        "stdio ".dimmed(),
        command,
        args.dimmed()
    )
}

pub(crate) fn mcp_saved_remote_line(name: &str, url: &str) -> String {
    format!(
        "{} MCP server '{}' saved (remote {url}).",
        "✓".green(),
        name.cyan()
    )
}

pub(crate) fn mcp_saved_stdio_line(name: &str, command: &str, args: &str) -> String {
    format!(
        "{} MCP server '{}' saved (stdio {command} {args}).",
        "✓".green(),
        name.cyan()
    )
}

pub(crate) fn mcp_removed_line(name: &str) -> String {
    format!("{} MCP server '{}' removed.", "✓".green(), name.cyan())
}

pub(crate) fn mcp_not_found_line(name: &str) -> String {
    format!("{} MCP server '{}' not found.", "✗".red(), name.cyan())
}

pub(crate) async fn cmd_mcp(cmd: &McpCmd) -> anyhow::Result<()> {
    let mut config = Config::load()?;

    match cmd {
        McpCmd::Serve { tools, cwd } => {
            use std::sync::Arc;
            use whycodes_core::types::PermissionSet;
            use whycodes_tools::executor::ToolExecutor;
            use whycodes_tools::profile::ToolProfile;

            let profile = ToolProfile::parse(tools);
            let working_dir = cwd.clone().unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".into())
            });
            let permissions = PermissionSet {
                allow_file_writes: true,
                allow_network: true,
                allow_shell: true,
                ..Default::default()
            };
            // Restrictive defaults: shell still risk-gated only when used via agent.
            // MCP serve runs tools directly — document risk; prefer core profile.
            let executor = Arc::new(ToolExecutor::new());
            eprintln!(
                "whycodes mcp serve — profile={} cwd={} (stdio JSON-RPC)",
                profile.as_str(),
                working_dir
            );
            whycodes_mcp::run_stdio_server(executor, permissions, profile, working_dir).await?;
            return Ok(());
        }
        McpCmd::List => {
            if config.mcp_servers.is_empty() {
                println!("{} No MCP servers configured.", "🔌".bold());
                println!();
                println!("Add one:");
                println!("  whycodes mcp add <name> <command> [--args \"arg1 arg2\"]");
                println!("  whycodes mcp add <name> --url https://mcp.example.com/mcp");
                println!("  whycodes mcp add <name> --url https://host/sse --type sse");
            } else {
                println!("{} Configured MCP servers:", "🔌".bold());
                for (name, server) in &config.mcp_servers {
                    if let Some(url) = &server.url {
                        let kind = server
                            .transport
                            .map(|t| format!("{t:?}").to_lowercase())
                            .unwrap_or_else(|| "auto".into());
                        println!("{}", mcp_remote_line(name, &kind, url));
                    } else {
                        let cmd = server.command.as_deref().unwrap_or("?");
                        println!("{}", mcp_stdio_line(name, cmd, &server.args));
                    }
                }
            }
        }
        McpCmd::Add {
            name,
            command,
            args,
            url,
            transport,
            headers,
        } => {
            let transport_kind = parse_mcp_transport(transport.as_deref())?;
            let header_map = parse_mcp_headers(headers)?;

            if url.is_none() && command.is_none() {
                anyhow::bail!("provide either a local <command> or --url <endpoint>");
            }
            if url.is_some() && command.is_some() {
                anyhow::bail!("use either a local <command> or --url, not both");
            }

            let arg_vec: Vec<String> = args
                .as_deref()
                .map(|s| s.split_whitespace().map(|a| a.to_string()).collect())
                .unwrap_or_default();

            let server = whycodes_config::McpServerConfig {
                transport: transport_kind,
                command: command.clone(),
                args: arg_vec.clone(),
                env: None,
                cwd: None,
                url: url.clone(),
                headers: header_map,
            };
            server
                .resolved_transport()
                .map_err(|e| anyhow::anyhow!(e))?;

            config.mcp_servers.insert(name.clone(), server);
            config.save()?;

            if let Some(url) = url {
                println!("{}", mcp_saved_remote_line(name, url));
            } else {
                println!(
                    "{}",
                    mcp_saved_stdio_line(
                        name,
                        command.as_deref().unwrap_or("?"),
                        &arg_vec.join(" ")
                    )
                );
            }
        }
        McpCmd::Remove { name } => {
            if config.mcp_servers.remove(name).is_some() {
                config.save()?;
                println!("{}", mcp_removed_line(name));
            } else {
                eprintln!("{}", mcp_not_found_line(name));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
