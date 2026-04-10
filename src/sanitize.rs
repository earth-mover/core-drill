//! String sanitization for untrusted data from cloud repos.
use strip_ansi_escapes::strip;

pub fn sanitize(s: &str) -> String {
    // strip_ansi_escapes::strip uses a VTE parser that consumes \t and \n
    // as control actions. Protect them with placeholders, strip ANSI, then restore.
    // Use Private Use Area codepoints as sentinels — won't appear in real data.
    const TAB_SENTINEL: &str = "\u{E000}";
    const NL_SENTINEL: &str = "\u{E001}";
    let protected = s.replace('\t', TAB_SENTINEL).replace('\n', NL_SENTINEL);
    let stripped = strip(&protected);
    let cleaned: String = String::from_utf8_lossy(&stripped)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    cleaned
        .replace(TAB_SENTINEL, "\t")
        .replace(NL_SENTINEL, "\n")
}

pub fn sanitize_truncate(s: &str, max_len: usize) -> String {
    let s = sanitize(s);
    if s.chars().count() > max_len {
        let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", truncated)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_normal_text() {
        assert_eq!(sanitize("hello world"), "hello world");
    }

    #[test]
    fn preserves_unicode() {
        assert_eq!(sanitize("data \u{00d7} 100"), "data \u{00d7} 100");
    }

    #[test]
    fn strips_ansi_csi_color() {
        assert_eq!(sanitize("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strips_ansi_csi_cursor() {
        assert_eq!(sanitize("\x1b[2Jhello"), "hello");
    }

    #[test]
    fn strips_osc_sequence() {
        assert_eq!(sanitize("\x1b]0;evil title\x07text"), "text");
    }

    #[test]
    fn strips_control_chars() {
        assert_eq!(sanitize("a\x00b\x01c\x7fd"), "abcd");
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        assert_eq!(sanitize("line1\nline2\tcol"), "line1\nline2\tcol");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(sanitize_truncate("short", 100), "short");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = sanitize_truncate(&long, 50);
        assert!(result.chars().count() <= 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_with_ansi() {
        let s = format!("\x1b[31m{}\x1b[0m", "x".repeat(100));
        let result = sanitize_truncate(&s, 50);
        assert!(result.chars().count() <= 50);
        assert!(!result.contains('\x1b'));
    }
}
