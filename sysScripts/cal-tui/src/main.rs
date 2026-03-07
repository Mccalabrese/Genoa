mod app;
mod engine;
mod model;
mod ui;

use std::io;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use app::{App, InputMode};
use chrono::Duration; // For moving days

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Initialize App
    let mut app = App::new();

    // 3. Run Event Loop
    let res = run_app(&mut terminal, &mut app);

    // 4. Restore Terminal (Even if we crash)
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}
fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui::ui(f, app))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => {
                    app.quit();
                    return Ok(());
                }
                KeyCode::Right => {
                    // Go to next day
                    app.current_date = app.current_date + Duration::days(1);
                }
                KeyCode::Left => {
                    // Go to previous day
                    app.current_date = app.current_date - Duration::days(1);
                }
                _ => {}
            }
        }
    }
}


//Comment to testupdater
