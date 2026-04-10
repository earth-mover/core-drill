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
