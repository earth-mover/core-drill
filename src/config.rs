use std::collections::BTreeMap;
use std::path::PathBuf;

use color_eyre::Result;
use serde::{Deserialize, Serialize};

/// Application config, loaded from `~/.config/core-drill/config.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub aliases: BTreeMap<String, Alias>,

    /// Extra Python packages to include in generated scripts (PEP 723 deps).
    /// These are added to every `core-drill script` output alongside
    /// icechunk/arraylake, zarr, and xarray.
    ///
    /// Example in config.toml:
    ///   script_deps = ["matplotlib", "pandas"]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub script_deps: Vec<String>,
}

/// A saved repo alias — short name that expands to a full repo reference
/// with optional storage overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    /// Full repo reference (path, URL, or al:org/repo)
    pub repo: String,
    /// Cloud storage region
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Storage endpoint URL (S3-compatible services)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    /// Use anonymous (unsigned) requests
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub anonymous: bool,
    /// Arraylake API endpoint (for non-production environments)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arraylake_api: Option<String>,
}

/// Path to the config file: `~/.config/core-drill/config.toml`
pub fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| color_eyre::eyre::eyre!("Cannot determine config directory"))?;
    Ok(dir.join("core-drill").join("config.toml"))
}

/// Load config from disk. Returns default config if the file doesn't exist.
pub fn load() -> Result<Config> {
    let path = config_path()?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let config: Config = toml::from_str(&contents)
                .map_err(|e| color_eyre::eyre::eyre!("Failed to parse {}: {e}", path.display()))?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => color_eyre::eyre::bail!("Failed to read {}: {e}", path.display()),
    }
}

/// Save config to disk, creating parent directories as needed.
pub fn save(config: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(config)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to serialize config: {e}"))?;
    std::fs::write(&path, contents)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to write {}: {e}", path.display()))?;
    Ok(())
}

/// Look up an alias by name. Returns None if no alias matches.
pub fn resolve_alias(name: &str) -> Result<Option<Alias>> {
    let config = load()?;
    Ok(config.aliases.get(name).cloned())
}
