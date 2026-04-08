//! Sanitize untrusted strings before display in the TUI.
//!
//! Icechunk repos from the cloud are untrusted. Metadata could contain
//! terminal escape sequences, ANSI codes, or control characters that
//! could hijack the terminal or confuse the display.

/// Strip control characters and ANSI escape sequences from a string.
/// Preserves printable Unicode, spaces, newlines, and tabs.
pub fn sanitize(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of an escape sequence — consume it
            if chars.peek() == Some(&'[') {
                // CSI sequence: ESC [ ... (parameters) final_byte (0x40-0x7E)
                chars.next(); // consume '['
                loop {
                    match chars.next() {
                        Some(c) if ('\x40'..='\x7e').contains(&c) => break,
                        Some(_) => continue,
                        None => break,
                    }
                }
            } else if chars.peek() == Some(&']') {
                // OSC sequence: ESC ] ... ST (ESC \ or BEL)
                chars.next(); // consume ']'
                loop {
                    match chars.next() {
                        Some('\x07') => break,            // BEL terminator
                        Some('\x1b') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next(); // consume '\'
                            }
                            break;
                        }
                        Some(_) => continue,
                        None => break,
                    }
                }
            } else {
                // Other escape: ESC + single char (e.g., ESC D, ESC M)
                chars.next(); // consume the next char
            }
        } else if ch == '\n' || ch == '\t' {
            result.push(ch);
        } else if ch.is_control() {
            // Strip all other control characters (U+0000-U+001F except \n \t, U+007F, etc.)
            continue;
        } else {
            result.push(ch);
        }
    }

    result
}

/// Sanitize and truncate to a max length, appending "..." if truncated.
/// `max_len` is measured in bytes for simplicity; truncation respects char boundaries.
pub fn sanitize_truncate(s: &str, max_len: usize) -> String {
    let s = sanitize(s);
    if s.len() <= max_len {
        return s;
    }
    let suffix = "...";
    let target = max_len.saturating_sub(suffix.len());
    // Find the last char boundary at or before `target`
    let end = s.floor_char_boundary(target);
    format!("{}{}", &s[..end], suffix)
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
        assert!(result.len() <= 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_with_ansi() {
        let s = format!("\x1b[31m{}\x1b[0m", "x".repeat(100));
        let result = sanitize_truncate(&s, 50);
        assert!(result.len() <= 50);
        assert!(!result.contains('\x1b'));
    }
}
