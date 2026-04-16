//! Sway Workspace Fetcher
//!
//! A minimal IPC client that queries the Sway Window Manager for the currently focused workspace.
//! Designed for use in status bars (like Waybar) or shell scripts that need context awareness
//! of the window manager's state.

use anyhow::{Context, Result};
use swayipc::Connection;

fn main() -> Result<()> {
    // 1. Establish IPC Connection
    // Connects to the Unix socket defined in the $SWAYSOCK environment variable.
    // Use the `swayipc` crate to abstract the low-level JSON-IPC protocol.
    let mut connection =
        Connection::new().context("Failed to connect to sway IPC. Is sway running?")?;

    // 2. Query Compositor State
    // Synchronously fetch the list of all active workspaces.
    let workspaces = connection
        .get_workspaces()
        .context("Failed to fetch workspaces")?;

    // 3. Filter & Extract
    // Use a functional iterator chain to find the single workspace marked as focused.
    let focused_name = workspaces
        .into_iter()
        .find(|ws| ws.focused) // Predicate: Is this the active one?
        .map(|ws| ws.name) // Transform: I only care about the name string
        .unwrap_or_else(|| "?".to_string()); // Fallback for transient states (e.g. during startup)
    // 4. Output
    // Print strictly to stdout so this binary can be used as a `custom/script` source in Waybar.
    println!("{}", focused_name);

    Ok(())
}
