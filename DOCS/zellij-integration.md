# Zellij Integration

## Problem
Ctrl+hjkl is used for both internal pane navigation in core-drill AND zellij pane navigation. They conflict.

## Solution (inspired by swaits/zellij-nav.nvim)

Same approach as neovim's zellij-nav plugin:
1. Detect zellij via `ZELLIJ` environment variable
2. On Ctrl+hjkl: first try to move within core-drill's panes
3. If already at an edge (no internal pane in that direction), shell out to `zellij action move-focus <direction>` to let zellij handle it

## Detection
- `ZELLIJ` env var is set (value "0") when running inside zellij
- `ZELLIJ_SESSION_NAME` contains the session name

## Edge Cases
- Sidebar is leftmost → Ctrl+h passes to zellij
- Detail is rightmost → Ctrl+l passes to zellij
- Top panes (Sidebar/Detail) → Ctrl+k passes to zellij
- Bottom pane → Ctrl+j passes to zellij
- When bottom panel is hidden, Sidebar/Detail are also bottommost

## Implementation
```rust
fn is_in_zellij() -> bool {
    std::env::var("ZELLIJ").is_ok()
}

fn zellij_move_focus(direction: &str) {
    // Fire and forget — don't block the UI
    std::process::Command::new("zellij")
        .args(["action", "move-focus", direction])
        .spawn()
        .ok();
}
```

Sources:
- https://github.com/swaits/zellij-nav.nvim
