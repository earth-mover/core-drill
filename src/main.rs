mod app;
mod cli;
mod component;
mod repo;
mod store;
mod theme;
mod tui;
mod ui;

use clap::Parser;
use cli::Cli;
use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Open the repository
    let overrides = repo::StorageOverrides {
        region: cli.region.clone(),
        endpoint_url: cli.endpoint_url.clone(),
    };
    let repository = repo::open(&cli.repo, &overrides).await?;

    match cli.output {
        Some(_format) => {
            // Structured output mode — no TUI
            todo!("Structured output mode")
        }
        None => {
            // Interactive TUI mode
            let data_store = store::DataStore::new(repository);
            let mut app = app::App::new(data_store, cli.repo.clone());
            app.load_initial_data();
            tui::run(app).await?;
        }
    }

    Ok(())
}
