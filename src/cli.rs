use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};

/// core-drill: A terminal UI for inspecting Icechunk V2 repositories
///
/// Explore branches, tags, snapshots, node trees, and metadata
/// in both local and remote Icechunk repositories. Supports
/// interactive TUI mode and structured JSON output for scripting.
#[derive(Parser, Debug)]
#[command(name = "core-drill", version, about, long_about)]
pub struct Cli {
    /// Path, URL, or Arraylake reference to an Icechunk repository
    ///
    /// Required for TUI, REPL, and --output modes. Optional for --serve
    /// (use the `open` tool to connect on demand).
    ///
    /// Examples:
    ///   ./my-repo                    Local filesystem
    ///   s3://bucket/prefix           AWS S3
    ///   gs://bucket/prefix           Google Cloud Storage
    ///   az://container/prefix        Azure Blob Storage
    ///   https://host/path            HTTP (read-only)
    ///   al:myorg/myrepo              Arraylake (credentials from ~/.arraylake/token.json)
    #[arg(value_name = "REPO", add = ArgValueCompleter::new(complete_repo))]
    pub repo: Option<String>,

    /// Cloud storage region (e.g., us-east-1)
    ///
    /// Can also be passed as a query parameter in the URL:
    ///   s3://bucket/prefix?region=us-east-1
    #[arg(long)]
    pub region: Option<String>,

    /// Storage endpoint URL (for S3-compatible services like MinIO, R2)
    #[arg(long)]
    pub endpoint_url: Option<String>,

    /// Use anonymous (unsigned) requests for cloud storage
    ///
    /// Skips credential lookup, useful for public repos.
    /// Equivalent to s3://bucket/prefix?anonymous=true
    #[arg(long, alias = "anon")]
    pub anonymous: bool,

    /// Arraylake API endpoint (used when REPO is an al:org/repo reference).
    /// If not set, uses the arraylake crate default.
    #[arg(long)]
    pub arraylake_api: Option<String>,

    /// Output format for non-interactive use
    ///
    /// When set, disables the interactive TUI and prints results
    /// to stdout in the specified format. Useful for scripting
    /// and agent/LLM consumption.
    #[arg(short, long, value_enum)]
    pub output: Option<OutputFormat>,

    /// Start a persistent REPL session (repo stays open, reads commands from stdin)
    ///
    /// Each line is a subcommand (e.g. "tree -p /data", "log -n 5").
    /// Responses are separated by "---END---" markers. Output format
    /// defaults to markdown; override per-session with --output.
    /// Ideal for agents that make multiple queries against a cloud repo.
    #[arg(long)]
    pub repl: bool,

    /// Start as an MCP (Model Context Protocol) server on stdio
    ///
    /// Exposes repository inspection as MCP tools for AI agents.
    /// No repo argument needed — use the `open` tool to connect.
    ///   claude mcp add --transport stdio core-drill -- core-drill --serve
    #[arg(long)]
    pub serve: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Output format for structured (non-interactive) mode
#[derive(Debug, Clone, ValueEnum, serde::Serialize)]
pub enum OutputFormat {
    /// JSON output (ideal for jq and scripts)
    Json,
    /// Markdown output (ideal for LLM agents — compact, expressive)
    Md,
    /// Human-readable table output
    Table,
}

/// Targeted inspection commands (optional — default is interactive TUI)
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show repository overview: status, branches, tags, snapshot count
    Info,

    /// List all branches with their target snapshots
    Branches,

    /// List all tags with their target snapshots
    Tags,

    /// Show snapshot history with ancestry
    Log {
        /// Branch or tag to show history for (default: main)
        #[arg(short, long, default_value = "main")]
        r#ref: String,

        /// Maximum number of snapshots to show
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },

    /// Browse the node tree at a given ref
    Tree {
        /// Branch, tag, or snapshot ID to inspect
        #[arg(short, long, default_value = "main")]
        r#ref: String,

        /// Path prefix to filter (e.g. /root/group1)
        #[arg(short, long)]
        path: Option<String>,
    },

    /// Show operations log (mutation history)
    OpsLog {
        /// Maximum number of entries to show
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },

    /// Set up tab completion (subcommands, flags, and alias names)
    ///
    /// Auto-detects your shell and appends the setup line to your
    /// shell config (~/.zshrc, ~/.bashrc, ~/.config/fish/config.fish).
    InstallCompletions {
        /// Shell to generate completions for (auto-detected from $SHELL if omitted)
        shell: Option<clap_complete::Shell>,
    },

    /// Manage saved repo aliases
    ///
    /// Aliases let you refer to frequently-used repositories by short names.
    /// Stored in ~/.config/core-drill/config.toml.
    ///
    /// Examples:
    ///   core-drill alias add era5 s3://icechunk-public-data/v1/era5_weatherbench2 --anonymous
    ///   core-drill alias list
    ///   core-drill era5              # opens the aliased repo
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
}

/// Subcommands for managing repo aliases
#[derive(Subcommand, Debug)]
pub enum AliasCommand {
    /// List all saved aliases
    #[command(alias = "ls")]
    List,

    /// Save a new alias (or update an existing one)
    ///
    /// Storage flags (--region, --anonymous, --endpoint-url) are saved
    /// with the alias and applied automatically when it's used.
    Add {
        /// Short name for the alias
        name: String,

        /// Full repo reference (path, URL, or al:org/repo)
        repo: String,

        /// Cloud storage region
        #[arg(long)]
        region: Option<String>,

        /// Storage endpoint URL (for S3-compatible services)
        #[arg(long)]
        endpoint_url: Option<String>,

        /// Use anonymous (unsigned) requests
        #[arg(long, alias = "anon")]
        anonymous: bool,

        /// Arraylake API endpoint (for dev/staging environments)
        #[arg(long)]
        arraylake_api: Option<String>,
    },

    /// Remove a saved alias
    #[command(alias = "rm")]
    Remove {
        /// Name of the alias to remove
        name: String,
    },
}

/// Dynamic completer for the REPO positional arg — suggests saved alias names.
fn complete_repo(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let prefix = current.to_string_lossy();
    let Ok(cfg) = crate::config::load() else {
        return vec![];
    };
    cfg.aliases
        .into_iter()
        .filter(|(name, _)| name.starts_with(prefix.as_ref()))
        .map(|(name, alias)| {
            CompletionCandidate::new(name).help(Some(alias.repo.into()))
        })
        .collect()
}
