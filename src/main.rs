mod app;
mod cli;
mod codegen;
mod component;
pub mod config;
mod fetch;
mod mcp;
mod multiplexer;
mod output;
mod repo;
pub mod sanitize;
pub mod search;
mod store;
mod theme;
mod tui;
mod ui;
pub mod util;

use std::sync::Arc;

use clap::{CommandFactory, Parser};
use cli::Cli;
use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Clean error display: no backtrace unless RUST_BACKTRACE is set
    color_eyre::config::HookBuilder::default()
        .display_env_section(false)
        .install()?;

    // Dynamic shell completions: when COMPLETE=<shell> is set, respond
    // with completions (including alias names) and exit immediately.
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();

    // Suppress icechunk's internal ERROR logs by default — they fire during
    // normal shutdown when manifest pre-load tasks are cancelled, and look
    // like failures to users. RUST_LOG can still override for debugging.
    let env_filter = tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,icechunk=off".to_string()),
    );
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();

    // Handle subcommands that don't need a repo connection
    match cli.command {
        Some(cli::Command::Alias { command }) => return run_alias_command(command),
        Some(cli::Command::ScriptDeps { command }) => return run_script_deps_command(command),
        Some(cli::Command::InstallCompletions { shell }) => return install_completions(shell),
        Some(cli::Command::SelfUpdate) => return self_update().await,
        Some(cli::Command::Script {
            ref filename,
            ref branch,
            ref snapshot,
            ref path,
            ref region,
            ref endpoint_url,
            anonymous: script_anon,
            ref arraylake_api,
            marimo,
            force,
            run,
        }) => {
            // Subcommand storage flags override top-level flags
            let merged_region = region.clone().or(cli.region.clone());
            let merged_endpoint = endpoint_url.clone().or(cli.endpoint_url.clone());
            let merged_anon = script_anon || cli.anonymous;
            let merged_api = arraylake_api.clone().or(cli.arraylake_api.clone());
            return run_script(
                cli.repo.as_deref(), filename, branch, snapshot.as_deref(), path.as_deref(),
                merged_region, merged_endpoint, merged_anon, merged_api.as_deref(),
                marimo, force, run,
            );
        }
        _ => {}
    }

    // For TUI mode (no --output, --serve, --repl), show loading screen while opening
    let is_tui = !cli.serve && !cli.repl && cli.output.is_none();

    if cli.serve {
        // MCP server mode: repo is optional — agents can `open` on demand
        let (repo, label) = if let Some(ref repo_str) = cli.repo {
            let (repository, repo_id) = open_repo(
                repo_str,
                cli.arraylake_api.as_deref(),
                &repo::StorageOverrides {
                    region: cli.region.clone(),
                    endpoint_url: cli.endpoint_url.clone(),
                    anonymous: cli.anonymous,
                },
            )
            .await?;
            (Some(repository), repo_id.display_short())
        } else {
            (None, String::new())
        };
        mcp::serve(repo, label).await?;
    } else if is_tui {
        let repo_str = cli
            .repo
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("A repo argument is required for TUI mode"))?;
        let api_url = cli.arraylake_api.clone();
        let overrides = repo::StorageOverrides {
            region: cli.region.clone(),
            endpoint_url: cli.endpoint_url.clone(),
            anonymous: cli.anonymous,
        };

        let label = format!("Opening {}...", repo_str);

        tui::run_with_loading(
            &label,
            async move {
                open_repo(&repo_str, api_url.as_deref(), &overrides).await
            },
            |(repository, repo_id)| {
                let data_store = store::DataStore::new(repository);
                app::App::new(data_store, repo_id)
            },
        )
        .await?;
    } else {
        // Non-TUI modes (REPL, --output): repo is required
        let repo_str = cli
            .repo
            .clone()
            .ok_or_else(|| color_eyre::eyre::eyre!("A repo argument is required"))?;

        let (repository, repo_id) = open_repo(
            &repo_str,
            cli.arraylake_api.as_deref(),
            &repo::StorageOverrides {
                region: cli.region.clone(),
                endpoint_url: cli.endpoint_url.clone(),
                anonymous: cli.anonymous,
            },
        )
        .await?;

        let display_label = repo_id.display_short();

        if cli.repl {
            let format = cli.output.unwrap_or(cli::OutputFormat::Md);
            output::run_repl(repository, format, &display_label).await?;
        } else if let Some(format) = cli.output {
            output::run(repository, format, cli.command, &display_label).await?;
        }
    }

    Ok(())
}

/// Open a repository from a string reference (local path, URL, alias, or arraylake ref).
/// Shared by main dispatch and MCP server's `open` tool.
///
/// If `repo_str` matches a saved alias, expands it and merges stored overrides
/// (CLI flags take precedence over alias values).
pub async fn open_repo(
    repo_str: &str,
    arraylake_api: Option<&str>,
    overrides: &repo::StorageOverrides,
) -> Result<(icechunk::Repository, app::RepoIdentity)> {
    // Try alias resolution — if the string matches an alias, expand it
    let (resolved, resolved_overrides, resolved_api);
    if let Some(alias) = config::resolve_alias(repo_str)? {
        resolved = alias.repo;
        // CLI flags override alias values
        resolved_overrides = repo::StorageOverrides {
            region: overrides.region.clone().or(alias.region),
            endpoint_url: overrides.endpoint_url.clone().or(alias.endpoint_url),
            anonymous: overrides.anonymous || alias.anonymous,
        };
        resolved_api = arraylake_api
            .map(|s| s.to_string())
            .or(alias.arraylake_api);
    } else {
        resolved = repo_str.to_string();
        resolved_overrides = repo::StorageOverrides {
            region: overrides.region.clone(),
            endpoint_url: overrides.endpoint_url.clone(),
            anonymous: overrides.anonymous,
        };
        resolved_api = arraylake_api.map(|s| s.to_string());
    }

    if looks_like_arraylake_ref(&resolved) {
        open_via_arraylake(&resolved, resolved_api.as_deref()).await
    } else {
        let repo = repo::open(&resolved, &resolved_overrides).await?;
        let identity = if resolved.contains("://") {
            app::RepoIdentity::S3 {
                url: resolved,
                region: resolved_overrides.region.clone(),
                endpoint_url: resolved_overrides.endpoint_url.clone(),
                anonymous: resolved_overrides.anonymous,
            }
        } else {
            app::RepoIdentity::Local { path: resolved }
        };
        Ok((repo, identity))
    }
}

/// Detect if a repo string is an Arraylake reference.
/// Explicit: `al:org/repo`. Implicit: `org/repo` that doesn't exist on disk.
fn looks_like_arraylake_ref(s: &str) -> bool {
    s.starts_with("al:")
}

/// Open a repo via Arraylake, handling credentials automatically.
/// Reads the OAuth token from ~/.arraylake/token.json.
/// API endpoint priority: explicit arg > ARRAYLAKE_SERVICE__URI env > crate default.
async fn open_via_arraylake(
    al_ref: &str,
    api_url: Option<&str>,
) -> Result<(icechunk::Repository, app::RepoIdentity)> {
    let ref_str = al_ref.strip_prefix("al:").unwrap_or(al_ref).trim_end_matches('/');
    let (org, repo_name) = ref_str.split_once('/').ok_or_else(|| {
        color_eyre::eyre::eyre!("Invalid Arraylake ref: expected 'al:org/repo', got '{al_ref}'")
    })?;

    // Resolve API endpoint: CLI flag > env var > crate default (None)
    // Accept shorthands: "dev" / "prod" expand to known Earthmover endpoints
    let env_api = std::env::var("ARRAYLAKE_SERVICE__URI").ok();
    let api_url_owned: Option<String> = api_url
        .or(env_api.as_deref())
        .map(crate::util::expand_api_url);
    let api_url = api_url_owned.as_deref();

    // Read token from ~/.arraylake/token.json
    let home = std::env::var("HOME")
        .map_err(|_| color_eyre::eyre::eyre!("Cannot determine home directory"))?;
    let token_path = std::path::PathBuf::from(home).join(".arraylake/token.json");

    let token_json = std::fs::read_to_string(&token_path).map_err(|e| {
        color_eyre::eyre::eyre!(
            "Cannot read Arraylake token at {}: {}. Run `arraylake auth login` first.",
            token_path.display(),
            e
        )
    })?;

    let token_data: serde_json::Value = serde_json::from_str(&token_json)?;
    let id_token = token_data["id_token"]
        .as_str()
        .ok_or_else(|| color_eyre::eyre::eyre!("No id_token in Arraylake token file"))?;

    let client = Arc::new(
        arraylake::ALClient::new(api_url.map(|s| s.to_string()), id_token.to_string())
            .map_err(|e| color_eyre::eyre::eyre!("Failed to create Arraylake client: {e}"))?,
    );

    // Fetch repo info first to get bucket details for display
    // Verify auth first with a quick user check
    if let Err(e) = client.get_current_user().await {
        color_eyre::eyre::bail!(
            "Arraylake authentication failed:\n  {e}\n\n\
             Run `arraylake auth login` to refresh your token."
        );
    }

    let repo_info = client.get_repo_info(org, repo_name).await.map_err(|e| {
        color_eyre::eyre::eyre!(
            "Repo '{ref_str}' not found:\n  {e}\n\n\
             Check the org/repo name, or verify access with `arraylake repo list {org}`."
        )
    })?;

    // Extract bucket metadata once — used for display and the identity struct
    let bucket_name = repo_info
        .bucket
        .as_ref()
        .map(|b| b.name.as_str())
        .unwrap_or("?");

    let region = repo_info
        .bucket
        .as_ref()
        .and_then(|b| {
            b.extra_config.get("region_name").map(|v| match v {
                arraylake::ALBucketExtraConfigValue::S(s) => s.clone(),
                arraylake::ALBucketExtraConfigValue::B(_) => "?".to_string(),
            })
        })
        .unwrap_or_else(|| "?".to_string());

    let platform = repo_info
        .bucket
        .as_ref()
        .map(|b| {
            let endpoint = b.extra_config.get("endpoint_url").and_then(|v| match v {
                arraylake::ALBucketExtraConfigValue::S(s) => Some(s.as_str()),
                _ => None,
            });
            let is_r2 = endpoint.is_some_and(|u| u.contains(".r2."));
            let is_tigris =
                endpoint.is_some_and(|u| u.contains("tigris.dev") || u.contains("t3.storage.dev"));
            match b.platform {
                arraylake::ALBucketPlatform::S3 => "S3".to_string(),
                arraylake::ALBucketPlatform::S3Compatible if is_r2 => "Cloudflare R2".to_string(),
                arraylake::ALBucketPlatform::S3Compatible if is_tigris => "Tigris".to_string(),
                arraylake::ALBucketPlatform::S3Compatible => "S3-compatible".to_string(),
                arraylake::ALBucketPlatform::Minio => "MinIO".to_string(),
                arraylake::ALBucketPlatform::GS => "GCS".to_string(),
            }
        })
        .unwrap_or_else(|| "?".to_string());

    tracing::info!("Arraylake: {org}/{repo_name}  →  {bucket_name} ({platform}, {region})");

    let storage = client
        .get_storage_for_repo(&repo_info)
        .await
        .map_err(|e| color_eyre::eyre::eyre!("Failed to get storage for '{ref_str}': {e}"))?;

    let repository = icechunk::Repository::open(None, storage, std::collections::HashMap::new())
        .await
        .map_err(|e| {
            color_eyre::eyre::eyre!("Failed to open Icechunk repo for '{ref_str}': {e}")
        })?;

    let identity = app::RepoIdentity::Arraylake {
        org: org.to_string(),
        repo: repo_name.to_string(),
        bucket: bucket_name.to_string(),
        platform,
        region,
        api_url: api_url.map(|s| s.to_string()),
    };
    Ok((repository, identity))
}

/// Handle `core-drill alias <subcommand>` — pure config file operations, no repo needed.
fn run_alias_command(command: cli::AliasCommand) -> Result<()> {
    match command {
        cli::AliasCommand::List => {
            let cfg = config::load()?;
            if cfg.aliases.is_empty() {
                println!("No aliases configured.");
                println!(
                    "\nAdd one with: core-drill alias add <name> <repo> [--anonymous] [--region <r>]"
                );
            } else {
                for (name, alias) in &cfg.aliases {
                    let mut flags = Vec::new();
                    if alias.anonymous {
                        flags.push("anonymous".to_string());
                    }
                    if let Some(ref r) = alias.region {
                        flags.push(format!("region={r}"));
                    }
                    if let Some(ref e) = alias.endpoint_url {
                        flags.push(format!("endpoint={e}"));
                    }
                    if let Some(ref api) = alias.arraylake_api {
                        flags.push(format!("api={api}"));
                    }
                    if flags.is_empty() {
                        println!("  {name:16} → {}", alias.repo);
                    } else {
                        println!("  {name:16} → {}  ({})", alias.repo, flags.join(", "));
                    }
                }
            }
        }
        cli::AliasCommand::Add {
            name,
            repo,
            region,
            endpoint_url,
            anonymous,
            arraylake_api,
        } => {
            let mut cfg = config::load()?;
            let is_update = cfg.aliases.contains_key(&name);
            cfg.aliases.insert(
                name.clone(),
                config::Alias {
                    repo: repo.clone(),
                    region,
                    endpoint_url,
                    anonymous,
                    arraylake_api,
                },
            );
            config::save(&cfg)?;
            if is_update {
                println!("Updated alias '{name}' → {repo}");
            } else {
                println!("Added alias '{name}' → {repo}");
            }
        }
        cli::AliasCommand::Remove { name } => {
            let mut cfg = config::load()?;
            if cfg.aliases.remove(&name).is_some() {
                config::save(&cfg)?;
                println!("Removed alias '{name}'");
            } else {
                color_eyre::eyre::bail!("No alias named '{name}'. Run `core-drill alias list` to see available aliases.");
            }
        }
    }
    Ok(())
}

/// Detect current shell from $SHELL, install completion eval line into rc file.
fn run_script_deps_command(command: cli::ScriptDepsCommand) -> Result<()> {
    match command {
        cli::ScriptDepsCommand::List => {
            let cfg = config::load()?;
            if cfg.script_deps.is_empty() {
                println!("No extra script dependencies configured.");
                println!(
                    "\nAdd some with: core-drill script-deps add matplotlib pandas"
                );
            } else {
                for dep in &cfg.script_deps {
                    println!("  {dep}");
                }
            }
        }
        cli::ScriptDepsCommand::Add { packages } => {
            let mut cfg = config::load()?;
            let mut added = Vec::new();
            for pkg in &packages {
                if !cfg.script_deps.contains(pkg) {
                    cfg.script_deps.push(pkg.clone());
                    added.push(pkg.as_str());
                }
            }
            config::save(&cfg)?;
            if added.is_empty() {
                println!("All packages already configured.");
            } else {
                println!("Added: {}", added.join(", "));
            }
        }
        cli::ScriptDepsCommand::Remove { packages } => {
            let mut cfg = config::load()?;
            let before = cfg.script_deps.len();
            cfg.script_deps.retain(|d| !packages.contains(d));
            let removed = before - cfg.script_deps.len();
            config::save(&cfg)?;
            if removed == 0 {
                println!("None of those packages were configured.");
            } else {
                println!("Removed {removed} package(s).");
            }
        }
    }
    Ok(())
}

fn install_completions(shell_override: Option<clap_complete::Shell>) -> Result<()> {
    use clap_complete::Shell;

    let shell = if let Some(s) = shell_override {
        s
    } else {
        detect_shell()?
    };

    let home = dirs::home_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Cannot determine home directory"))?;

    let (rc_path, eval_line) = match shell {
        Shell::Zsh => (
            home.join(".zshrc"),
            "source <(COMPLETE=zsh core-drill)",
        ),
        Shell::Bash => (
            home.join(".bashrc"),
            "source <(COMPLETE=bash core-drill)",
        ),
        Shell::Fish => (
            home.join(".config/fish/config.fish"),
            "COMPLETE=fish core-drill | source",
        ),
        _ => color_eyre::eyre::bail!(
            "Auto-install not supported for {shell:?}. Run `core-drill completions {shell:?}` and add the output to your shell config manually.",
        ),
    };

    // Check if already installed
    if rc_path.exists() {
        let contents = std::fs::read_to_string(&rc_path)?;
        if contents.contains("COMPLETE=") && contents.contains("core-drill") {
            println!("Already installed in {}", rc_path.display());
            return Ok(());
        }
    }

    // Append
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc_path)?;
    writeln!(file, "\n# core-drill tab completion")?;
    writeln!(file, "{eval_line}")?;

    println!("Added to {}", rc_path.display());
    println!("Restart your shell or run: source {}", rc_path.display());
    Ok(())
}

fn detect_shell() -> Result<clap_complete::Shell> {
    use clap_complete::Shell;
    let shell_env = std::env::var("SHELL").unwrap_or_default();
    if shell_env.contains("zsh") {
        Ok(Shell::Zsh)
    } else if shell_env.contains("bash") {
        Ok(Shell::Bash)
    } else if shell_env.contains("fish") {
        Ok(Shell::Fish)
    } else {
        color_eyre::eyre::bail!(
            "Cannot detect shell from $SHELL='{shell_env}'. Pass the shell explicitly: core-drill install-completions bash"
        )
    }
}

/// Extract human-readable code from a file for diffing.
/// For .ipynb, extracts the source lines from code cells.
/// For everything else, returns the content as-is.
fn extract_diffable(content: &str, filename: &str) -> String {
    if filename.ends_with(".ipynb") {
        if let Ok(nb) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(cells) = nb["cells"].as_array() {
                return cells
                    .iter()
                    .filter(|c| c["cell_type"].as_str() == Some("code"))
                    .filter(|c| {
                        // Skip hidden metadata cells (juv PEP 723 cell)
                        c["metadata"]["jupyter"]["source_hidden"].as_bool() != Some(true)
                    })
                    .filter_map(|c| {
                        c["source"].as_array().map(|lines| {
                            lines
                                .iter()
                                .filter_map(|l| l.as_str())
                                .collect::<String>()
                        })
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
        }
    }
    content.to_string()
}

fn print_truncated_diff(old: &str, new: &str) {
    let max_lines = 12;
    let mut shown = 0;
    for (old_line, new_line) in old.lines().zip(new.lines()) {
        if old_line != new_line {
            eprintln!("  \x1b[31m- {old_line}\x1b[0m");
            eprintln!("  \x1b[32m+ {new_line}\x1b[0m");
            shown += 1;
            if shown >= max_lines {
                eprintln!("  ...");
                break;
            }
        }
    }
    let old_len = old.lines().count();
    let new_len = new.lines().count();
    if old_len != new_len && shown < max_lines {
        eprintln!("  ({old_len} lines → {new_len} lines)");
    }
}

/// Generate a connection script and write it to a file. No network needed.
#[allow(clippy::too_many_arguments)]
fn run_script(
    repo_arg: Option<&str>,
    filename: &str,
    branch: &str,
    snapshot: Option<&str>,
    path: Option<&str>,
    region: Option<String>,
    endpoint_url: Option<String>,
    anonymous: bool,
    arraylake_api: Option<&str>,
    marimo: bool,
    force: bool,
    exec: bool,
) -> Result<()> {
    let repo_str = repo_arg
        .ok_or_else(|| color_eyre::eyre::eyre!("A repo argument is required for `script`"))?;

    // Infer format from extension + flags
    let format = if filename.ends_with(".ipynb") {
        codegen::ScriptFormat::Jupyter
    } else if filename.ends_with(".py") && marimo {
        codegen::ScriptFormat::Marimo
    } else if filename.ends_with(".py") {
        codegen::ScriptFormat::Python
    } else if filename.ends_with(".rs") {
        codegen::ScriptFormat::Rust
    } else {
        color_eyre::eyre::bail!(
            "Cannot infer language from '{filename}'.\n\
             Supported extensions: .py, .rs, .ipynb"
        );
    };

    // Resolve alias if applicable
    let (resolved, resolved_region, resolved_endpoint, resolved_anon) =
        if let Some(alias) = config::resolve_alias(repo_str)? {
            (
                alias.repo,
                region.or(alias.region),
                endpoint_url.or(alias.endpoint_url),
                anonymous || alias.anonymous,
            )
        } else {
            (repo_str.to_string(), region, endpoint_url, anonymous)
        };

    let resolved_api = arraylake_api.map(|s| s.to_string());
    let identity = app::RepoIdentity::from_url(&resolved, resolved_region, resolved_endpoint, resolved_anon, resolved_api);
    let ctx = codegen::CodeContext {
        branch: branch.to_string(),
        snapshot: snapshot.map(|s| s.to_string()),
        path: path.map(|p| p.to_string()),
    };

    let extra_deps = config::load().map(|c| c.script_deps).unwrap_or_default();
    let content = codegen::generate_script(&identity, &ctx, &format, &extra_deps);
    let dest = std::path::Path::new(filename);

    if dest.exists() {
        let existing = std::fs::read_to_string(dest)?;
        if existing != content && !force {
            eprintln!("\x1b[31merror:\x1b[0m File '{filename}' already exists with different content. Use --force to overwrite.\n");
            // For notebooks, diff the Python code content, not the raw JSON
            let old_text = extract_diffable(&existing, filename);
            let new_text = extract_diffable(&content, filename);
            print_truncated_diff(&old_text, &new_text);
            std::process::exit(1);
        }
        if existing != content {
            std::fs::write(dest, &content)?;
        }
    } else {
        std::fs::write(dest, &content)?;
    }

    if !exec {
        println!("Written to \x1b[1m{filename}\x1b[0m, run with:\n\n  {}\n", codegen::run_hint(&format, filename));
    }

    if exec {
        let commands = codegen::run_commands(&format, filename);
        for (prog, args) in &commands {
            let display = format!("{prog} {}", args.join(" "));
            match std::process::Command::new(prog).args(args).status() {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Try next fallback
                    continue;
                }
                Err(e) => {
                    eprintln!("\x1b[31merror:\x1b[0m Failed to run `{display}`: {e}");
                    std::process::exit(1);
                }
            }
        }
        // All commands failed to launch
        let names: Vec<&str> = commands.iter().map(|(p, _)| *p).collect();
        eprintln!(
            "\x1b[31merror:\x1b[0m None of [{}] found on PATH.\nInstall one, then run:\n  {}",
            names.join(", "),
            codegen::run_hint(&format, filename),
        );
        std::process::exit(1);
    }
    Ok(())
}

async fn self_update() -> Result<()> {
    let mut updater = axoupdater::AxoUpdater::new_for("core-drill");
    updater.load_receipt()?;

    if !updater.is_update_needed().await? {
        println!("Already up to date ({})", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    println!("Updating core-drill...");
    let result = updater.run().await?;
    if let Some(outcome) = result {
        println!("Updated to {}", outcome.new_version);
    }
    Ok(())
}
