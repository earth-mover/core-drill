use std::io::{self, Stdout};

use color_eyre::Result;
use crossterm::{
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{Terminal, prelude::CrosstermBackend};

use crate::app::App;
use crate::ui;

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Initialize the terminal for TUI mode
fn init() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
fn restore(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Run the TUI event loop
pub async fn run(mut app: App) -> Result<()> {
    let mut terminal = init()?;

    // Install panic hook that restores terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    let mut event_stream = EventStream::new();

    loop {
        // Drain any pending background responses before rendering
        app.drain_responses();

        terminal.draw(|frame| ui::render(&app, frame))?;

        // Wait for either a terminal event or a short timeout to stay responsive
        // to background data responses
        tokio::select! {
            maybe_event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event
                    && key.kind == KeyEventKind::Press {
                        app.handle_key(key);
                    }
            }
            // Short sleep so we loop back to drain responses and redraw (~60fps)
            _ = tokio::time::sleep(std::time::Duration::from_millis(16)) => {}
        }

        if app.should_quit {
            break;
        }
    }

    restore(&mut terminal)?;
    Ok(())
}
