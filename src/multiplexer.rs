//! Terminal multiplexer integration (zellij, tmux, etc.)
//!
//! When core-drill runs inside a terminal multiplexer and the user presses
//! Ctrl+hjkl at an edge pane (no internal pane in that direction), we pass
//! the navigation through to the multiplexer so it can switch to its own panes.
//!
//! Inspired by swaits/zellij-nav.nvim and christoomey/vim-tmux-navigator.

use std::sync::LazyLock;

/// Which multiplexer (if any) we're running inside
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Multiplexer {
    Zellij,
    Tmux,
    None,
}

/// Detected once at startup
static DETECTED: LazyLock<Multiplexer> = LazyLock::new(detect);

fn detect() -> Multiplexer {
    if std::env::var("ZELLIJ").is_ok() {
        Multiplexer::Zellij
    } else if std::env::var("TMUX").is_ok() {
        Multiplexer::Tmux
    } else {
        Multiplexer::None
    }
}

/// Which multiplexer are we inside?
pub fn detected() -> Multiplexer {
    *DETECTED
}

/// Pass focus to the multiplexer in the given direction.
/// Direction: "left", "right", "up", "down"
///
/// This is fire-and-forget — we don't block the UI.
pub fn move_focus(direction: &str) {
    match detected() {
        Multiplexer::Zellij => {
            std::process::Command::new("zellij")
                .args(["action", "move-focus", direction])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
        }
        Multiplexer::Tmux => {
            let tmux_dir = match direction {
                "left" => "-L",
                "right" => "-R",
                "up" => "-U",
                "down" => "-D",
                _ => return,
            };
            std::process::Command::new("tmux")
                .args(["select-pane", tmux_dir])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok();
        }
        Multiplexer::None => {
            // No multiplexer — nothing to pass through to
        }
    }
}
