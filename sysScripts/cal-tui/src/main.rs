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
use app::{App, InputMode, EditField, RecField};
use chrono::{Datelike, Duration, Weekday}; // For moving days
use crate::app::ViewMode;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 2. Initialize App
    let mut app = App::new();

    // --- CLI ARGUMENT PARSER ---
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    let mut target_id: Option<u32> = None;

    while i < args.len() {
        match args[i].as_str() {
            "--date" => {
                if i + 1 < args.len() {
                    if let Ok(date) = chrono::NaiveDate::parse_from_str(&args[i + 1], "%Y-%m-%d") {
                        app.current_date = date;
                    }
                    i += 1;
                }
            }
            "--select-id" => {
                if i + 1 < args.len() {
                    if let Ok(id) = args[i + 1].parse::<u32>() {
                        target_id = Some(id);
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // If they passed an ID, we need to find its row index in today's agenda
    if let Some(id) = target_id {
        let events = app.engine.get_appointments_on_day(app.current_date);
        // Find the position of the event with this ID
        if let Some(index) = events.iter().position(|e| e.id == id) {
            app.list_state.select(Some(index)); // Highlight it!
        }
    }
    // ---------------------------------

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

        if let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press {
                match app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('?') => {
                            app.show_help = !app.show_help;
                        }
                        KeyCode::Char('q') => {
                            app.quit();
                            return Ok(());
                        }
                        KeyCode::Char('t') => {
                            app.current_date = chrono::Utc::now().date_naive();
                            app.list_state.select(None);
                            app.set_status("Jumped to today");
                        }
                        KeyCode::Char('v') => {
                            app.view_mode = if app.view_mode == ViewMode::Day {
                                ViewMode::Week
                            } else {
                                ViewMode::Day
                            };
                            app.list_state.select(None); // Reset cursor on view change
                        }
                        KeyCode::Right => {
                            app.current_date += Duration::days(1);
                            app.list_state.select(None); // Clear selection on day change
                        }
                        KeyCode::Char('l') => {
                            app.current_date += Duration::days(1);
                            app.list_state.select(None);
                        }
                        KeyCode::Left => {
                            app.current_date -= Duration::days(1);
                            app.list_state.select(None); // Clear selection on day change
                        }
                        KeyCode::Char('h') => {
                            app.current_date -= Duration::days(1);
                            app.list_state.select(None);
                        }
                        KeyCode::Char('a') => {
                            app.input_mode = InputMode::Editing;
                            app.input_buffer.clear();
                        }
                    
                        // NEW: Scroll Down
                        KeyCode::Down => {
                            let events = app.engine.get_appointments_on_day(app.current_date);
                            if !events.is_empty() {
                                let i = match app.list_state.selected() {
                                    Some(i) => if i >= events.len() - 1 { 0 } else { i + 1 }, // Loop to top
                                    None => 0, // Start at index 0
                                };
                                app.list_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('j') => {
                            let events = app.engine.get_appointments_on_day(app.current_date);
                            if !events.is_empty() {
                                let i = match app.list_state.selected() {
                                    Some(i) => if i >= events.len() - 1 { 0 } else { i + 1 },
                                    None => 0,
                                };
                                app.list_state.select(Some(i));
                            }
                        }
                    
                        // NEW: Scroll Up
                        KeyCode::Up => {
                            let events = app.engine.get_appointments_on_day(app.current_date);
                            if !events.is_empty() {
                                let i = match app.list_state.selected() {
                                    Some(i) => if i == 0 { events.len() - 1 } else { i - 1 }, // Loop to bottom
                                    None => 0,
                                };
                                app.list_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('k') => {
                            let events = app.engine.get_appointments_on_day(app.current_date);
                            if !events.is_empty() {
                                let i = match app.list_state.selected() {
                                    Some(i) => if i == 0 { events.len() - 1 } else { i - 1 },
                                    None => 0,
                                };
                                app.list_state.select(Some(i));
                            }
                        }
                    
                        // NEW: Delete
                        KeyCode::Char('d') => {
                            if let Some(selected_idx) = app.list_state.selected() {
                                let events = app.engine.get_appointments_on_day(app.current_date);
                                let num_events = events.len();
                            
                                // Safety check to ensure we don't crash
                                if selected_idx < num_events {
                                    let id_to_remove = events[selected_idx].id; // Grab the ID of the selected event
                                    drop(events);
                                    app.engine.remove_appointment(id_to_remove); // Delete it from memory
                                    app.save(); // Save to disk immediately
                                    app.set_status("Appointment deleted");
                                
                                    // Adjust the cursor so it doesn't fall off the screen
                                    if selected_idx > 0 {
                                        app.list_state.select(Some(selected_idx - 1));
                                    } else if num_events > 1 {
                                        app.list_state.select(Some(0));
                                    } else {
                                        app.list_state.select(None); // List is now empty
                                    }
                                }
                            }
                        }
                        _ => {}
                    },

                    InputMode::Editing => match key.code {
                        // Switch Fields
                        KeyCode::Tab => {
                            app.active_field = match app.active_field {
                                EditField::Summary => EditField::StartTime,
                                EditField::StartTime => EditField::Duration,
                                EditField::Duration => EditField::IsRecurring,
                                EditField::IsRecurring => EditField::Summary, // Loop back to top
                            };
                        }
                        
                        // Toggle Checkbox
                        KeyCode::Char(' ') => {
                            if app.active_field == EditField::IsRecurring {
                                app.is_recurring = !app.is_recurring; // Flip true/false
                            } else if app.active_field == EditField::Summary {
                                app.input_buffer.push(' '); // Normal space if typing the name
                            }
                        }

                    // Spinners
                        KeyCode::Up => {
                            match app.active_field {
                                EditField::StartTime => app.time_minutes = (app.time_minutes + 30) % 1440,
                                EditField::Duration => if app.duration_minutes < 1440 { app.duration_minutes += 15; },
                                _ => {}
                            }
                        }
                        KeyCode::Down => {
                            match app.active_field {
                                EditField::StartTime => app.time_minutes = (app.time_minutes + 1440 - 30) % 1440,
                                EditField::Duration => if app.duration_minutes > 15 { app.duration_minutes -= 15; },
                                _ => {}
                            }
                        }

                        // Save OR Proceed to next screen
                        KeyCode::Enter => {
                            if !app.input_buffer.trim().is_empty() {
                                if app.is_recurring {
                                    // Switch to the next dialogue menu
                                    app.input_mode = InputMode::EditingRecurrence;
                                } else {
                                    // Save as a Singular Event
                                    let hours = app.time_minutes / 60;
                                    let mins = app.time_minutes % 60;
                                    let Some(parsed_time) = chrono::NaiveTime::from_hms_opt(hours, mins, 0) else {
                                        app.set_status("Invalid start time");
                                        continue;
                                    };
                                    let start_time = app.current_date.and_time(parsed_time).and_utc();
                                    let duration = chrono::Duration::minutes(app.duration_minutes as i64);

                                    let new_app = crate::model::Appointment {
                                        id: 0,
                                        summary: app.input_buffer.clone(),
                                        start: start_time,
                                        duration,
                                        rule: None, // No rule
                                        exceptions: vec![],
                                    };

                                    app.engine.add_appointment(new_app);
                                    app.save();
                                    app.set_status("Appointment saved");

                                    app.reset_form();
                                    app.input_mode = InputMode::Normal;
                                }
                            } else {
                                // If they left the name blank, just abort
                                app.reset_form();
                                app.input_mode = InputMode::Normal;
                            }
                        }

                        KeyCode::Esc => { 
                            app.reset_form();
                            app.input_mode = InputMode::Normal;
                        }
                        
                        // Route typing to the Summary box
                        KeyCode::Char(c) => { 
                            if app.active_field == EditField::Summary {
                                app.input_buffer.push(c);
                            }
                        }
                        
                        // Handle backspace
                        KeyCode::Backspace => { 
                            if app.active_field == EditField::Summary {
                                app.input_buffer.pop();
                            }
                        }
                        _ => {}
                    },
                    // NEW: Handle the second page (just a placeholder for now so it compiles)
                    InputMode::EditingRecurrence => match key.code {
                        KeyCode::Tab => {
                            app.active_rec_field = match app.active_rec_field {
                                RecField::Mon => RecField::Tue,
                                RecField::Tue => RecField::Wed,
                                RecField::Wed => RecField::Thu,
                                RecField::Thu => RecField::Fri,
                                RecField::Fri => RecField::Sat,
                                RecField::Sat => RecField::Sun,
                                RecField::Sun => RecField::EndToggle,
                                RecField::EndToggle => RecField::EndWeeks,
                                RecField::EndWeeks => RecField::Mon, // Loop back
                            };
                        }
                        KeyCode::Char(' ') => {
                            match app.active_rec_field {
                                RecField::Mon => app.rec_days[0] = !app.rec_days[0],
                                RecField::Tue => app.rec_days[1] = !app.rec_days[1],
                                RecField::Wed => app.rec_days[2] = !app.rec_days[2],
                                RecField::Thu => app.rec_days[3] = !app.rec_days[3],
                                RecField::Fri => app.rec_days[4] = !app.rec_days[4],
                                RecField::Sat => app.rec_days[5] = !app.rec_days[5],
                                RecField::Sun => app.rec_days[6] = !app.rec_days[6],
                                RecField::EndToggle => app.rec_end_date = !app.rec_end_date,
                                _ => {}
                            }
                        }
                        KeyCode::Up => {
                            if app.active_rec_field == RecField::EndWeeks {
                                app.rec_end_weeks += 1;
                            }
                        }
                        KeyCode::Down => {
                            if app.active_rec_field == RecField::EndWeeks && app.rec_end_weeks > 1 {
                                app.rec_end_weeks -= 1;
                            }
                        }
                        KeyCode::Enter => {
                            // 1. Calculate base start time & duration (same as normal mode)
                            let hours = app.time_minutes / 60;
                            let mins = app.time_minutes % 60;
                            let Some(parsed_time) = chrono::NaiveTime::from_hms_opt(hours, mins, 0) else {
                                app.set_status("Invalid start time");
                                continue;
                            };
                            let start_time = app.current_date.and_time(parsed_time).and_utc();
                            let duration = chrono::Duration::minutes(app.duration_minutes as i64);

                            // 2. Build the active days vector
                            let mut active_days = Vec::new();
                            let all_days = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];
                            for (i, &day) in all_days.iter().enumerate() {
                                if app.rec_days[i] {
                                    active_days.push(day);
                                }
                            }

                            if active_days.is_empty() {
                                active_days.push(start_time.weekday());
                                app.set_status("No recurrence days selected; defaulted to start weekday");
                            }

                            // 3. Calculate End Date (if checked)
                            let until_date = if app.rec_end_date {
                                Some(start_time + chrono::Duration::weeks(app.rec_end_weeks as i64))
                            } else {
                                None
                            };

                            // 4. Save the complex appointment!
                            let new_app = crate::model::Appointment {
                                id: 0,
                                summary: app.input_buffer.clone(),
                                start: start_time,
                                duration,
                                rule: Some(crate::model::Recurrence::Weekly {
                                    days: active_days,
                                    until: until_date,
                                }),
                                exceptions: vec![],
                            };

                            app.engine.add_appointment(new_app);
                            app.save();
                            app.set_status("Recurring appointment saved");

                            app.reset_form();
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Esc => {
                            app.reset_form();
                            app.input_mode = InputMode::Normal;
                        }
                        _ => {}
                    },
                }
            }
    }
}
