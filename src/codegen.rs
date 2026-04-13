//! Code snippet generation for the Connect detail tab.
//!
//! Given a `RepoIdentity` and session context, produces ready-to-paste
//! Python and Rust snippets that open the same repository.

use crate::app::RepoIdentity;

/// Live context for code generation — reflects current TUI selections.
pub struct CodeContext {
    /// Currently active branch
    pub branch: String,
    /// If the user is viewing a specific snapshot (overrides branch for session)
    pub snapshot: Option<String>,
    /// Selected tree node path (None or "/" = root, no path filter)
    pub path: Option<String>,
}

/// Output format for script generation.
pub enum ScriptFormat {
    Python,
    Rust,
    Jupyter,
    Marimo,
}

/// Generate `(python, rust)` code snippets for the TUI Connect tab.
pub fn generate(identity: &RepoIdentity, ctx: &CodeContext) -> (String, String) {
    let (py_setup, rs_setup) = storage_setup(identity);
    let is_arraylake = matches!(identity, RepoIdentity::Arraylake { .. });
    let deps = all_deps(is_arraylake, &[]);
    (
        assemble_python(&py_setup, ctx, &deps),
        assemble_rust(&rs_setup, ctx, is_arraylake),
    )
}

/// Generate a script in the requested format, including extra user-configured deps.
pub fn generate_script(
    identity: &RepoIdentity,
    ctx: &CodeContext,
    format: &ScriptFormat,
    extra_deps: &[String],
) -> String {
    let (py_setup, rs_setup) = storage_setup(identity);
    let is_arraylake = matches!(identity, RepoIdentity::Arraylake { .. });
    let deps = all_deps(is_arraylake, extra_deps);
    match format {
        ScriptFormat::Python => assemble_python(&py_setup, ctx, &deps),
        ScriptFormat::Rust => assemble_rust(&rs_setup, ctx, is_arraylake),
        ScriptFormat::Jupyter => assemble_jupyter(&py_setup, ctx, &deps),
        ScriptFormat::Marimo => assemble_marimo(&py_setup, ctx, &deps),
    }
}

/// Returns the run command hint for a given format + filename.
pub fn run_hint(format: &ScriptFormat, filename: &str) -> String {
    match format {
        ScriptFormat::Python => format!("uv run {filename}"),
        ScriptFormat::Rust => format!("cargo script {filename}"),
        ScriptFormat::Jupyter => format!("juv run {filename}"),
        ScriptFormat::Marimo => format!("marimo edit {filename}"),
    }
}

/// Returns commands to try in order (native tool first, uvx fallback).
/// Each entry is (program, args).
pub fn run_commands(format: &ScriptFormat, filename: &str) -> Vec<(&'static str, Vec<String>)> {
    match format {
        ScriptFormat::Python => vec![
            ("uv", vec!["run".into(), filename.into()]),
        ],
        ScriptFormat::Rust => vec![
            ("cargo", vec!["script".into(), filename.into()]),
        ],
        ScriptFormat::Jupyter => vec![
            ("juv", vec!["run".into(), filename.into()]),
            ("uvx", vec!["juv".into(), "run".into(), filename.into()]),
        ],
        ScriptFormat::Marimo => vec![
            ("marimo", vec!["edit".into(), filename.into()]),
            ("uvx", vec!["marimo".into(), "edit".into(), filename.into()]),
        ],
    }
}

fn storage_setup(identity: &RepoIdentity) -> (String, String) {
    match identity {
        RepoIdentity::Local { path } => local_storage(path),
        RepoIdentity::S3 {
            url,
            region,
            endpoint_url,
            anonymous,
        } => url_storage(url, region.as_deref(), endpoint_url.as_deref(), *anonymous),
        RepoIdentity::Arraylake { org, repo, api_url, .. } => arraylake_storage(org, repo, api_url.as_deref()),
    }
}

// ── Assembly (shared tail) ──────────────────────────────────────────────────

fn assemble_python(setup: &str, ctx: &CodeContext, deps_list: &[String]) -> String {
    let dep_strs: Vec<&str> = deps_list.iter().map(|s| s.as_str()).collect();
    let deps = pep723_header(&dep_strs);
    let session_line = python_session_line(ctx);
    let path_line = python_path_line(ctx);

    format!(
        "{deps}{setup}
{session_line}
root = zarr.open(session.store){path_line}
# ds = xr.open_zarr(session.store, consolidated=False)
"
    )
}

fn assemble_rust(setup: &str, ctx: &CodeContext, is_arraylake: bool) -> String {
    // Arraylake has no Rust SDK — return as-is (it's already a comment block)
    if is_arraylake {
        return setup.to_string();
    }

    let version_info = if let Some(ref snap) = ctx.snapshot {
        let short = truncate_id(snap);
        format!("&VersionInfo::SnapshotId(\"{short}\".into())")
    } else {
        format!("&VersionInfo::BranchTipRef(\"{}\".into())", ctx.branch)
    };

    format!(
        r#"{setup}
    let repo = Repository::open(None, storage, Default::default()).await?;
    let session = repo
        .readonly_session({version_info})
        .await?;
    Ok(())
}}"#
    )
}

fn assemble_jupyter(setup: &str, ctx: &CodeContext, deps_list: &[String]) -> String {
    let session_line = python_session_line(ctx);
    let path_line = match ctx.path {
        Some(ref p) if p != "/" && !p.is_empty() => format!("group = root[\"{p}\"]\n"),
        _ => String::new(),
    };

    // Build the cell source as individual lines (ipynb format)
    let code = format!(
        "{setup}\n{session_line}\n\nroot = zarr.open(session.store)\n{path_line}# ds = xr.open_zarr(session.store, consolidated=False)\n"
    );
    let source_lines = ipynb_source(&code);

    // PEP 723 metadata in a hidden code cell (juv convention)
    let dep_strs: Vec<&str> = deps_list.iter().map(|s| s.as_str()).collect();
    let meta_header = pep723_header(&dep_strs);
    let meta_lines = ipynb_source(&meta_header);

    let notebook = serde_json::json!({
        "cells": [
            {
                "cell_type": "code",
                "id": "core-drill-meta",
                "execution_count": null,
                "metadata": {
                    "jupyter": {
                        "source_hidden": true
                    }
                },
                "outputs": [],
                "source": meta_lines
            },
            {
                "cell_type": "code",
                "id": "core-drill-main",
                "execution_count": null,
                "metadata": {},
                "outputs": [],
                "source": source_lines
            }
        ],
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3",
                "language": "python",
                "name": "python3"
            }
        },
        "nbformat": 4,
        "nbformat_minor": 5
    });

    serde_json::to_string_pretty(&notebook).unwrap()
}

fn assemble_marimo(setup: &str, ctx: &CodeContext, deps_list: &[String]) -> String {
    let mut all: Vec<&str> = deps_list.iter().map(|s| s.as_str()).collect();
    if !all.contains(&"marimo") {
        all.push("marimo");
    }
    let deps = pep723_header(&all);

    let session_line = python_session_line(ctx);

    let path_line = match ctx.path {
        Some(ref p) if p != "/" && !p.is_empty() => format!("\n    group = root[\"{p}\"]"),
        _ => String::new(),
    };

    // Indent the setup lines for the cell body
    let indented_setup = setup
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{deps}import marimo

app = marimo.App()


@app.cell
def _():
{indented_setup}
    {session_line}

    root = zarr.open(session.store){path_line}
    return (root,)


if __name__ == \"__main__\":
    app.run()
"
    )
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn pep723_header(deps: &[&str]) -> String {
    let mut lines =
        String::from("# /// script\n# requires-python = \">=3.12\"\n# dependencies = [\n");
    for dep in deps {
        lines.push_str(&format!("#     \"{dep}\",\n"));
    }
    lines.push_str("# ]\n# ///\n");
    lines
}

/// Convert a multiline string to ipynb source format (array of strings with \n).
fn ipynb_source(text: &str) -> Vec<serde_json::Value> {
    let lines: Vec<&str> = text.lines().collect();
    let last = lines.len().saturating_sub(1);
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i < last {
                serde_json::Value::String(format!("{line}\n"))
            } else {
                serde_json::Value::String(line.to_string())
            }
        })
        .collect()
}

/// Truncate a snapshot ID for display (first 12 chars).
fn truncate_id(id: &str) -> &str {
    crate::output::format::truncate(id, 12)
}

/// Python session line based on branch or snapshot.
fn python_session_line(ctx: &CodeContext) -> String {
    if let Some(ref snap) = ctx.snapshot {
        let short = truncate_id(snap);
        format!("session = repo.readonly_session(snapshot=\"{short}\")")
    } else {
        format!("session = repo.readonly_session(branch=\"{}\")", ctx.branch)
    }
}

/// Python path access line (empty string if no path selected).
fn python_path_line(ctx: &CodeContext) -> String {
    match ctx.path {
        Some(ref p) if p != "/" && !p.is_empty() => format!("\ngroup = root[\"{p}\"]"),
        _ => String::new(),
    }
}

/// Build the full dependency list: base deps + user-configured extras.
fn all_deps(is_arraylake: bool, extra: &[String]) -> Vec<String> {
    let base: &[&str] = if is_arraylake {
        &["arraylake", "zarr", "xarray"]
    } else {
        &["icechunk", "zarr", "xarray"]
    };
    let mut deps: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    for dep in extra {
        if !deps.iter().any(|d| d == dep) {
            deps.push(dep.clone());
        }
    }
    deps
}

// ── Backend-specific storage setup ──────────────────────────────────────────
//
// Each returns (python_setup, rust_setup).
// Python setup: imports + storage creation + `repo = ...` line.
// Rust setup: use statements + fn main header + storage creation (no repo/session).

fn url_storage(
    url: &str,
    region: Option<&str>,
    endpoint_url: Option<&str>,
    anonymous: bool,
) -> (String, String) {
    let parsed = url::Url::parse(url);

    match parsed.as_ref().map(|u| u.scheme()) {
        Ok("s3") => {
            let u = parsed.unwrap();
            let bucket = u.host_str().unwrap_or("my-bucket");
            let prefix = u.path().trim_start_matches('/');
            s3_storage(bucket, prefix, region, endpoint_url, anonymous)
        }
        Ok("gs") => {
            let u = parsed.unwrap();
            let bucket = u.host_str().unwrap_or("my-bucket");
            let prefix = u.path().trim_start_matches('/');
            gcs_storage(bucket, prefix)
        }
        Ok("az") => {
            let u = parsed.unwrap();
            let account = u.host_str().unwrap_or("account");
            let path = u.path().trim_start_matches('/');
            let (container, prefix) = path.split_once('/').unwrap_or((path, ""));
            azure_storage(account, container, prefix)
        }
        Ok("http" | "https") => http_storage(url),
        _ => s3_storage(url, "", region, endpoint_url, anonymous),
    }
}

// ── Local ───────────────────────────────────────────────────────────────────

fn local_storage(path: &str) -> (String, String) {
    let python = format!(
        "import icechunk
import zarr
import xarray as xr

storage = icechunk.local_filesystem_storage(\"{path}\")
repo = icechunk.Repository.open(storage=storage)
"
    );

    let rust = format!(
        r#"use icechunk::{{Repository, VersionInfo}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let storage = icechunk::new_local_filesystem_storage("{path}").await?;"#
    );

    (python, rust)
}

// ── S3 ──────────────────────────────────────────────────────────────────────

fn s3_storage(
    bucket: &str,
    prefix: &str,
    region: Option<&str>,
    endpoint_url: Option<&str>,
    anonymous: bool,
) -> (String, String) {
    // Python — each line includes its own trailing comma
    let mut py_lines = vec![format!("    bucket=\"{bucket}\",")];
    if !prefix.is_empty() {
        py_lines.push(format!("    prefix=\"{prefix}\","));
    }
    match region {
        Some(r) => py_lines.push(format!("    region=\"{r}\",")),
        None => {
            py_lines.push("    # NOTE: region is a guess — update if needed".to_string());
            py_lines.push("    region=\"us-east-1\",".to_string());
        }
    }
    if let Some(e) = endpoint_url {
        py_lines.push(format!("    endpoint_url=\"{e}\","));
    }
    if anonymous {
        py_lines.push("    anonymous=True,".to_string());
    }
    let py_args_str = py_lines.join("\n");

    let python = format!(
        "import icechunk
import zarr
import xarray as xr

storage = icechunk.s3_storage(
{py_args_str}
)
repo = icechunk.Repository.open(storage=storage)
"
    );

    // Rust
    let mut config_lines = Vec::new();
    match region {
        Some(r) => config_lines.push(format!("        region: Some(\"{r}\".into()),")),
        None => config_lines.push("        region: Some(\"us-east-1\".into()), // default guess — update if needed".to_string()),
    }
    if let Some(e) = endpoint_url {
        config_lines.push(format!("        endpoint_url: Some(\"{e}\".into()),"));
    }
    if anonymous {
        config_lines.push("        anonymous: true,".to_string());
    }
    config_lines.push("        ..Default::default()".to_string());
    let config_body = config_lines.join("\n");

    let prefix_arg = if prefix.is_empty() {
        "None".to_string()
    } else {
        format!("Some(\"{prefix}\".into())")
    };

    let creds_arg = if anonymous {
        "Some(S3Credentials::Anonymous)"
    } else {
        "None"
    };

    let creds_import = if anonymous {
        "use icechunk::storage::{S3Options, S3Credentials, new_s3_storage};"
    } else {
        "use icechunk::storage::{S3Options, new_s3_storage};"
    };

    let rust = format!(
        r#"use icechunk::{{Repository, VersionInfo}};
{creds_import}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let config = S3Options {{
{config_body}
    }};
    let storage = new_s3_storage(
        config,
        "{bucket}".into(),
        {prefix_arg},
        {creds_arg},
    )?;"#
    );

    (python, rust)
}

// ── GCS ─────────────────────────────────────────────────────────────────────

fn gcs_storage(bucket: &str, prefix: &str) -> (String, String) {
    let mut py_lines = vec![format!("    bucket=\"{bucket}\",")];
    if !prefix.is_empty() {
        py_lines.push(format!("    prefix=\"{prefix}\","));
    }
    let py_args_str = py_lines.join("\n");

    let python = format!(
        "import icechunk
import zarr
import xarray as xr

storage = icechunk.gcs_storage(
{py_args_str}
)
repo = icechunk.Repository.open(storage=storage)
"
    );

    let prefix_arg = if prefix.is_empty() {
        "None".to_string()
    } else {
        format!("Some(\"{prefix}\".into())")
    };

    let rust = format!(
        r#"use icechunk::{{Repository, VersionInfo}};
use icechunk::storage::new_gcs_storage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let storage = new_gcs_storage(
        Default::default(),
        "{bucket}".into(),
        {prefix_arg},
        None,
    )?;"#
    );

    (python, rust)
}

// ── Azure ───────────────────────────────────────────────────────────────────

fn azure_storage(account: &str, container: &str, prefix: &str) -> (String, String) {
    let mut py_lines = vec![
        format!("    account=\"{account}\","),
        format!("    container=\"{container}\","),
    ];
    if !prefix.is_empty() {
        py_lines.push(format!("    prefix=\"{prefix}\","));
    }
    let py_args_str = py_lines.join("\n");

    let python = format!(
        "import icechunk
import zarr
import xarray as xr

storage = icechunk.azure_storage(
{py_args_str}
)
repo = icechunk.Repository.open(storage=storage)
"
    );

    let prefix_arg = if prefix.is_empty() {
        "None".to_string()
    } else {
        format!("Some(\"{prefix}\".into())")
    };

    let rust = format!(
        r#"use icechunk::{{Repository, VersionInfo}};
use icechunk::storage::new_azure_blob_storage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let storage = new_azure_blob_storage(
        Default::default(),
        "{account}".into(),
        "{container}".into(),
        {prefix_arg},
        None,
    )?;"#
    );

    (python, rust)
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn http_storage(url: &str) -> (String, String) {
    let python = format!(
        "import icechunk
import zarr
import xarray as xr

storage = icechunk.http_storage(url=\"{url}\")
repo = icechunk.Repository.open(storage=storage)
"
    );

    let rust = format!(
        r#"use icechunk::{{Repository, VersionInfo}};
use icechunk::storage::new_http_storage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let storage = new_http_storage("{url}")?;"#
    );

    (python, rust)
}

// ── Arraylake ───────────────────────────────────────────────────────────────

fn arraylake_storage(org: &str, repo: &str, api_url: Option<&str>) -> (String, String) {
    let client_line = match api_url {
        Some(url) => format!("client = arraylake.Client(service_uri=\"{url}\")"),
        None => "client = arraylake.Client()".to_string(),
    };
    let python = format!(
        "import arraylake
import zarr
import xarray as xr

{client_line}
repo = client.get_repo(\"{org}/{repo}\")
"
    );

    let rust = format!(
        "// Arraylake Rust SDK not yet available.\n\
         // Use the Python SDK or the arraylake CLI:\n\
         //   arraylake repo list {org}\n\
         //   arraylake data info {org}/{repo}"
    );

    (python, rust)
}
