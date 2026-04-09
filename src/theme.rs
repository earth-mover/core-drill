use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

/// Shared visual theme for all components
#[allow(dead_code)]
pub struct Theme {
    pub border: Style,
    pub border_focused: Style,
    pub border_type: BorderType,

    pub text: Style,
    pub text_dim: Style,
    pub text_bold: Style,

    pub selected: Style,
    pub selected_inactive: Style,
    pub active: Style,

    // Semantic: diff colors
    pub added: Style,
    pub removed: Style,
    pub modified: Style,

    // Semantic: ref types
    pub branch: Style,
    pub tag: Style,
    pub snapshot_id: Style,
    pub timestamp: Style,

    // Semantic: node types
    pub group_icon: Style,
    pub array_icon: Style,

    // Status
    pub loading: Style,
    pub error: Style,
    pub status_ok: Style,
}

// Earthmover brand colors (from design/brand-tokens.yaml)
mod colors {
    use ratatui::prelude::Color;

    // Primary palette
    pub const VIOLET: Color = Color::Rgb(155, 87, 250);
    pub const LIME: Color = Color::Rgb(192, 227, 50);

    // Secondary palette
    pub const ICECHUNK_BLUE: Color = Color::Rgb(94, 196, 247);
    pub const GREEN: Color = Color::Rgb(49, 212, 149);
    pub const ORANGE: Color = Color::Rgb(255, 158, 13);
    pub const RED: Color = Color::Rgb(255, 101, 84);

    // UI support
    pub const DARK_GRAY: Color = Color::Rgb(120, 120, 120);
    pub const LIGHT_VIOLET: Color = Color::Rgb(195, 150, 249);
    pub const LIGHT_GRAY: Color = Color::Rgb(245, 245, 245);
}

impl Default for Theme {
    fn default() -> Self {
        use colors::*;

        Self {
            border: Style::default().fg(DARK_GRAY),
            border_focused: Style::default().fg(ICECHUNK_BLUE),
            border_type: BorderType::Rounded,

            text: Style::default().fg(LIGHT_GRAY),
            text_dim: Style::default().fg(DARK_GRAY),
            text_bold: Style::default().fg(LIGHT_GRAY).add_modifier(Modifier::BOLD),

            selected: Style::default().fg(LIME).add_modifier(Modifier::BOLD),
            selected_inactive: Style::default().fg(LIGHT_GRAY),
            active: Style::default()
                .fg(ICECHUNK_BLUE)
                .add_modifier(Modifier::UNDERLINED),

            added: Style::default().fg(GREEN),
            removed: Style::default().fg(RED),
            modified: Style::default().fg(ORANGE),

            branch: Style::default().fg(ICECHUNK_BLUE),
            tag: Style::default().fg(VIOLET),
            snapshot_id: Style::default().fg(DARK_GRAY),
            timestamp: Style::default().fg(LIGHT_VIOLET),

            group_icon: Style::default().fg(ORANGE),
            array_icon: Style::default().fg(ICECHUNK_BLUE),

            loading: Style::default().fg(ORANGE),
            error: Style::default().fg(RED),
            status_ok: Style::default().fg(GREEN),
        }
    }
}

/// Build a bordered block with consistent theme styling
pub fn panel<'a>(title: &'a str, focused: bool, theme: &Theme) -> Block<'a> {
    Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(theme.border_type)
        .border_style(if focused {
            theme.border_focused
        } else {
            theme.border
        })
}

/// Render a loading placeholder
pub fn loading_widget(theme: &Theme) -> Paragraph<'static> {
    Paragraph::new("  Loading...").style(theme.loading)
}

/// Render an error message with classification hint
pub fn error_widget<'a>(msg: &'a str, theme: &Theme) -> Paragraph<'a> {
    let kind = crate::store::classify_error(msg);
    let hint = match kind {
        crate::store::ErrorKind::Auth => "  (credentials may be expired — press R to retry)",
        crate::store::ErrorKind::Network => "  (network issue — press R to retry)",
        crate::store::ErrorKind::NotFound => "  (not found — press R to retry)",
        crate::store::ErrorKind::Other => "  (press R to retry)",
    };
    Paragraph::new(Line::from(vec![
        Span::styled(format!("  Error: {}", msg), theme.error),
        Span::styled(hint, theme.text_dim),
    ]))
}
