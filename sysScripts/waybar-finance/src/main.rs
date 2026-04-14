//! Application entry point.
//!
//! Handles command-line argument parsing, configuration loading, and
//! dispatching the application to either "Waybar Mode" (one-shot JSON output)
//! or "TUI Mode" (interactive terminal UI).

mod app;
mod config;
mod network;
mod ui;

use anyhow::Result;
use app::App;
use clap::Parser;
use config::{get_config_path, load_config};
use network::run_waybar_mode;
use ratatui::style::Color;
use reqwest::Client;
use ui::run_tui;

/// Command line arguments.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Launch the interactive Terminal User Interface (TUI).
    /// If omitted, outputs JSON for Waybar.
    #[arg(short, long)]
    tui: bool,
}
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the HTTP client with a persistent cookie store.
    // I strictly define the User-Agent to mimic a real browser, which prevents
    // 403 Forbidden errors from the Yahoo Finance API.
    let client = Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                     AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/106 Safari/537.36",
        )
        .cookie_store(true)
        .build()?;
    // "Warm up" the client by hitting the homepage.
    // This is required to acquire the initial session cookies and "crumb"
    // needed for subsequent API calls to the v7/v10 endpoints.
    let _ = client.get("https://finance.yahoo.com").send().await;
    let args = Args::parse();
    // Load user configuration (API keys, watchlist)
    let config_path = get_config_path()?;
    let config = load_config(&config_path)?;
    let mut app = App::new(config, String::from("Ready"), Color::Gray, None);
    // Dispatch based on mode
    if args.tui {
        println!("Initializing TUI mode...");
        run_tui(&client, &mut app).await?
    } else {
        run_waybar_mode(&client).await?;
    }
    Ok(())
}
