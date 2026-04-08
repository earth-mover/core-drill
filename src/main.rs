mod app;
mod cli;
mod repo;
mod tui;
mod ui;

use clap::Parser;
use cli::Cli;
use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Open the repository
    let repository = repo::open(&cli.repo).await?;

    match cli.output {
        Some(_format) => {
            // Structured output mode — no TUI
            todo!("Structured output mode")
        }
        None => {
            // Interactive TUI mode
            let mut app = app::App::new(repository);
            app.load_repo_info().await?;
            tui::run(app).await?;
        }
    }

    Ok(())
}
