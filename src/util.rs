/// Extract parent path: "/a/b/c" -> "/a/b", "/a" -> "/", "x" -> "/"
pub fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(idx) => &path[..idx],
        None => "/",
    }
}

/// Extract leaf name: "/a/b/c" -> "c", "/" -> ""
pub fn leaf_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

/// Expand Arraylake API URL shorthands: "dev", "prod", bare hostname.
pub fn expand_api_url(url: &str) -> String {
    match url {
        "dev" => "https://dev.api.earthmover.io".to_string(),
        "prod" => "https://api.earthmover.io".to_string(),
        s if !s.contains("://") => format!("https://{s}"),
        s => s.to_string(),
    }
}
