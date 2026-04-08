use clap::{Parser, Subcommand, ValueEnum};

/// core-drill: A terminal UI for inspecting Icechunk V2 repositories
///
/// Explore branches, tags, snapshots, node trees, and metadata
/// in both local and remote Icechunk repositories. Supports
/// interactive TUI mode and structured JSON output for scripting.
#[derive(Parser, Debug)]
#[command(name = "core-drill", version, about, long_about)]
pub struct Cli {
    /// Path or URL to an Icechunk repository
    ///
    /// Examples:
    ///   ./my-repo                    Local filesystem
    ///   s3://bucket/prefix           AWS S3
    ///   gs://bucket/prefix           Google Cloud Storage
    ///   az://container/prefix        Azure Blob Storage
    ///   https://host/path            HTTP (read-only)
    #[arg(value_name = "REPO")]
    pub repo: String,

    /// Cloud storage region (e.g., us-east-1)
    ///
    /// Can also be passed as a query parameter in the URL:
    ///   s3://bucket/prefix?region=us-east-1
    #[arg(long)]
    pub region: Option<String>,

    /// Storage endpoint URL (for S3-compatible services like MinIO, R2)
    #[arg(long)]
    pub endpoint_url: Option<String>,

    /// Output format for non-interactive use
    ///
    /// When set, disables the interactive TUI and prints results
    /// to stdout in the specified format. Useful for scripting
    /// and agent/LLM consumption.
    #[arg(short, long, value_enum)]
    pub output: Option<OutputFormat>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Output format for structured (non-interactive) mode
#[derive(Debug, Clone, ValueEnum, serde::Serialize)]
pub enum OutputFormat {
    /// JSON output (ideal for jq, scripts, and LLM agents)
    Json,
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
}
