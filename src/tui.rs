use std::io::{self, Stdout};
use std::future::Future;

use color_eyre::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, prelude::*};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme::Theme;
use crate::ui;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Initialize the terminal for TUI mode
fn init() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
fn restore(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

// ─── Loading screen ─────────────────────────────────────────

const DRILL_FRAMES: &[&str] = &[
    r#"
        ╦
        ║
       ╔╩╗
       ║ ║
       ╠═╣
       ║/║
       ╠═╣
       ║\║
       ╠═╣
       ║/║
       ╚╤╝
        │
        ▽
"#,
    r#"
        ╦
        ║
       ╔╩╗
       ║ ║
       ╠═╣
       ║\║
       ╠═╣
       ║/║
       ╠═╣
       ║\║
       ╚╤╝
        │
        ▽
"#,
    r#"
        ╦
        ║
       ╔╩╗
       ║ ║
       ╠═╣
       ║/║
       ╠═╣
       ║\║
       ╠═╣
       ║/║
       ╚╤╝
        ╎
        ▽
"#,
    r#"
        ╦
        ║
       ╔╩╗
       ║ ║
       ╠═╣
       ║\║
       ╠═╣
       ║/║
       ╠═╣
       ║\║
       ╚╤╝
        ╎
        ▽
"#,
];

/// Show an animated loading screen while a future resolves.
/// Returns the future's result, or Err if the user presses q/Esc.
pub async fn loading_screen<F, T>(
    terminal: &mut Tui,
    label: &str,
    future: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let theme = Theme::default();
    let mut event_stream = EventStream::new();
    let mut frame_idx: usize = 0;

    tokio::pin!(future);

    loop {
        let drill_art = DRILL_FRAMES[frame_idx % DRILL_FRAMES.len()];
        let status = format!("  {}  ", label);

        terminal.draw(|f| {
            let area = f.area();

            // Center the drill art vertically and horizontally
            let art_lines: Vec<&str> = drill_art.lines().filter(|l| !l.is_empty()).collect();
            let art_height = art_lines.len() as u16;
            let total_height = art_height + 4; // art + spacing + status + hint

            let y_offset = area.height.saturating_sub(total_height) / 2;

            for (i, line) in art_lines.iter().enumerate() {
                let x = area.width.saturating_sub(line.len() as u16) / 2;
                let y = y_offset + i as u16;
                if y < area.height {
                    let line_area = Rect::new(x, y, line.len() as u16, 1);
                    f.render_widget(
                        Paragraph::new(*line).style(theme.branch),
                        line_area,
                    );
                }
            }

            // Status line below the drill
            let status_y = y_offset + art_height + 1;
            if status_y < area.height {
                let status_x = area.width.saturating_sub(status.len() as u16) / 2;
                f.render_widget(
                    Paragraph::new(status.as_str()).style(theme.loading),
                    Rect::new(status_x, status_y, status.len() as u16, 1),
                );
            }

            // Hint
            let hint = "q to quit";
            let hint_y = status_y + 2;
            if hint_y < area.height {
                let hint_x = area.width.saturating_sub(hint.len() as u16) / 2;
                f.render_widget(
                    Paragraph::new(hint).style(theme.text_dim),
                    Rect::new(hint_x, hint_y, hint.len() as u16, 1),
                );
            }
        })?;

        tokio::select! {
            result = &mut future => {
                return result;
            }
            maybe_event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind == KeyEventKind::Press
                        && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    {
                        color_eyre::eyre::bail!("Cancelled by user");
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                frame_idx += 1;
            }
        }
    }
}

/// Initialize the terminal, show loading screen during repo open, then run the main TUI.
pub async fn run_with_loading<F, T>(
    label: &str,
    open_future: F,
    build_app: impl FnOnce(T) -> App,
) -> Result<()>
where
    F: Future<Output = Result<T>>,
{
    let mut terminal = init()?;

    // Install panic hook that restores terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    // Show loading screen while opening the repo
    let result = match loading_screen(&mut terminal, label, open_future).await {
        Ok(r) => r,
        Err(e) => {
            restore(&mut terminal)?;
            return Err(e);
        }
    };

    // Clear the loading screen before starting the main TUI
    terminal.clear()?;

    // Build the app and start the main TUI
    let mut app = build_app(result);
    app.load_initial_data();

    let mut event_stream = EventStream::new();

    loop {
        app.drain_responses();
        terminal.draw(|frame| ui::render(&mut app, frame))?;

        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        app.handle_key(key);
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        app.handle_mouse(mouse);
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(16)) => {}
        }

        if app.should_quit {
            break;
        }
    }

    restore(&mut terminal)?;
    Ok(())
}
