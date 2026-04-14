use crate::app::{App, EditField, InputMode, RecField, ViewMode};
use chrono::Datelike;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

pub fn ui(f: &mut Frame, app: &mut App) {
    // 1. Split screen: Header (Top) and Body (Bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header height
            Constraint::Min(0),    // Remaining space
            Constraint::Length(1), // Keyhint line
        ])
        .split(f.area());

    // 2. Render Header
    let title = format!(" 📅 Calendar TUI - {} ", app.current_date);
    let header = Paragraph::new(title)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, chunks[0]);

    // 3. Render Body (Day View vs Week View)
    if app.view_mode == ViewMode::Day {
        // --- EXISTING DAY VIEW ---
        let events = app.engine.get_appointments_on_day(app.current_date);
        let items: Vec<ListItem> = if events.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "No appointments.",
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            events
                .iter()
                .map(|evt| {
                    let time_str = evt.start.format("%H:%M").to_string();
                    ListItem::new(Line::from(format!("{} - {}", time_str, evt.summary)))
                })
                .collect()
        };

        let events_list = List::new(items)
            .block(Block::default().title(" Agenda ").borders(Borders::ALL))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        f.render_stateful_widget(events_list, chunks[1], &mut app.list_state);
    } else {
        // --- NEW RESPONSIVE WEEK VIEW ---
        // 1. Media Query: Is the terminal wide enough for 7 vertical columns?
        let is_wide = chunks[1].width > 100;

        // 2. Split into Calendar (Top 80%) and Details Pane (Bottom 20%)
        let week_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
            .split(chunks[1]);

        // 3. Slice the Calendar area into 7 equal blocks
        let day_chunks = Layout::default()
            .direction(if is_wide {
                Direction::Horizontal
            } else {
                Direction::Vertical
            })
            .constraints([
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
                Constraint::Ratio(1, 7),
            ])
            .split(week_chunks[0]);

        // 4. Find the Monday of the current week
        let days_from_mon = app.current_date.weekday().num_days_from_monday();
        let monday = app.current_date - chrono::Duration::days(days_from_mon as i64);

        let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let mut details_text = String::from(" Select an event to view details...");

        // 5. Draw the 7 days
        for i in 0..7 {
            let current_loop_day = monday + chrono::Duration::days(i);
            let events = app.engine.get_appointments_on_day(current_loop_day);

            // Is this the column our cursor is currently inside?
            let is_active_column = i as u32 == days_from_mon;
            let border_color = if is_active_column {
                Color::Yellow
            } else {
                Color::Reset
            };

            // Build the list items for this specific day
            let items: Vec<ListItem> = events
                .iter()
                .map(|evt| {
                    let time_str = evt.start.format("%H:%M").to_string();
                    ListItem::new(Line::from(format!("{} {}", time_str, evt.summary)))
                })
                .collect();

            let mut list_widget = List::new(items).block(
                Block::default()
                    .title(format!(
                        " {} {} ",
                        day_names[i as usize],
                        current_loop_day.format("%m/%d")
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            ); // Yellow border if active!

            // If this is the active column, render it with the stateful cursor
            if is_active_column {
                list_widget = list_widget
                    .highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol(">");

                f.render_stateful_widget(list_widget, day_chunks[i as usize], &mut app.list_state);

                // Grab data for the details pane!
                if let Some(idx) = app.list_state.selected()
                    && idx < events.len()
                {
                    let evt = events[idx];
                    let dur = evt.duration.num_minutes();
                    details_text = format!(
                        " Event: {}\n Time:  {}\n Dur:   {}h {}m\n ID:    {}",
                        evt.summary,
                        evt.start.format("%A, %B %e at %H:%M"),
                        dur / 60,
                        dur % 60,
                        evt.id
                    );
                }
            } else {
                // Not active, just render it statically
                f.render_widget(list_widget, day_chunks[i as usize]);
            }
        }

        // 6. Render the Details Pane at the bottom
        let details_block = Paragraph::new(details_text)
            .style(Style::default().fg(Color::Cyan))
            .block(Block::default().borders(Borders::ALL).title(" Details "));
        f.render_widget(details_block, week_chunks[1]);
    }
    // 4. Render Popup (If Editing)
    if app.input_mode == InputMode::Editing {
        let area = centered_rect(60, 80, f.area());
        f.render_widget(ratatui::widgets::Clear, area);

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .title(" New Appointment (Tab: Next, Enter: Save/Continue) ");
        f.render_widget(outer_block, area);

        let popup_chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Summary
                Constraint::Length(5), // Time
                Constraint::Length(5), // Duration
                Constraint::Length(3), // Recurring Checkbox
            ])
            .split(area);

        // Field 1: Summary
        let sum_color = if app.active_field == EditField::Summary {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let sum_block = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(sum_color))
            .block(Block::default().borders(Borders::ALL).title(" Event Name "));
        f.render_widget(sum_block, popup_chunks[0]);

        // Field 2: Start Time (Spinner)
        let time_color = if app.active_field == EditField::StartTime {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let hours = app.time_minutes / 60;
        let mins = app.time_minutes % 60;
        let time_text = if app.active_field == EditField::StartTime {
            format!(" ▲ \n{:02}:{:02}\n ▼ ", hours, mins)
        } else {
            format!("\n{:02}:{:02}", hours, mins)
        };
        let time_block = Paragraph::new(time_text)
            .style(Style::default().fg(time_color))
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Start Time (Up/Down) "),
            );
        f.render_widget(time_block, popup_chunks[1]);

        // Field 3: Duration (Spinner)
        let dur_color = if app.active_field == EditField::Duration {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let d_hours = app.duration_minutes / 60;
        let d_mins = app.duration_minutes % 60;
        let dur_text = if app.active_field == EditField::Duration {
            format!(" ▲ \n{}h {}m\n ▼ ", d_hours, d_mins)
        } else {
            format!("\n{}h {}m", d_hours, d_mins)
        };
        let dur_block = Paragraph::new(dur_text)
            .style(Style::default().fg(dur_color))
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Duration (Up/Down) "),
            );
        f.render_widget(dur_block, popup_chunks[2]);

        // Field 4: Recurring Checkbox
        let rec_color = if app.active_field == EditField::IsRecurring {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let check_mark = if app.is_recurring {
            "[X] Yes"
        } else {
            "[ ] No"
        };
        let rec_text = format!(" {}", check_mark); // Pad with a space

        let rec_block = Paragraph::new(rec_text)
            .style(Style::default().fg(rec_color))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Recurring? (Space to toggle) "),
            );
        f.render_widget(rec_block, popup_chunks[3]);
    }

    // 5. Render Recurrence Popup (Page 2)
    if app.input_mode == InputMode::EditingRecurrence {
        let area = centered_rect(70, 60, f.area()); // Slightly wider to fit the days
        f.render_widget(ratatui::widgets::Clear, area);

        let outer_block = Block::default()
            .borders(Borders::ALL)
            .title(" Step 2: Recurrence Rules (Tab: Move, Space: Toggle, Enter: Save) ");
        f.render_widget(outer_block, area);

        let popup_chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Days of week
                Constraint::Length(3), // End Date Toggle
                Constraint::Length(5), // Weeks Spinner
            ])
            .split(area);

        // Chunk 1: Days of the Week (Inline coloring!)
        let day_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let fields = [
            RecField::Mon,
            RecField::Tue,
            RecField::Wed,
            RecField::Thu,
            RecField::Fri,
            RecField::Sat,
            RecField::Sun,
        ];

        let mut day_spans = Vec::new();
        for i in 0..7 {
            let color = if app.active_rec_field == fields[i] {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            let check = if app.rec_days[i] { "[X]" } else { "[ ]" };
            day_spans.push(Span::styled(
                format!("{} {}   ", check, day_names[i]),
                Style::default().fg(color),
            ));
        }

        let days_block = Paragraph::new(Line::from(day_spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Days "),
        );
        f.render_widget(days_block, popup_chunks[0]);

        // Chunk 2: End Date Toggle
        let toggle_color = if app.active_rec_field == RecField::EndToggle {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let check_end = if app.rec_end_date {
            "[X] Yes"
        } else {
            "[ ] No (Runs forever)"
        };
        let end_toggle_block = Paragraph::new(format!(" {}", check_end))
            .style(Style::default().fg(toggle_color))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Has End Date? "),
            );
        f.render_widget(end_toggle_block, popup_chunks[1]);

        // Chunk 3: End Date Weeks (Spinner)
        let weeks_color = if app.active_rec_field == RecField::EndWeeks {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let weeks_text = if app.active_rec_field == RecField::EndWeeks {
            format!(" ▲ \n{} Weeks\n ▼ ", app.rec_end_weeks)
        } else {
            format!("\n{} Weeks", app.rec_end_weeks)
        };
        let weeks_block = Paragraph::new(weeks_text)
            .style(Style::default().fg(weeks_color))
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Duration of Recurrence "),
            );

        // Only show the spinner clearly if the toggle is actually checked
        if app.rec_end_date {
            f.render_widget(weeks_block, popup_chunks[2]);
        }
    }
    // 6. Render the Footer Keyhints
    // Match the text to whatever the user is currently doing
    let base_hint = match app.input_mode {
        InputMode::Normal => {
            " [q]uit | [a]dd | [d]elete | [v]iew | [t]oday | [h/l or ←/→] day | [j/k or ↑/↓] select | [?] help "
        }
        InputMode::Editing => {
            " [Tab] next field | [Space] toggle | [↑/↓] adjust time | [Enter] save/next | [Esc] cancel "
        }
        InputMode::EditingRecurrence => {
            " [Tab] move | [Space] check | [↑/↓] weeks | [Enter] save | [Esc] cancel "
        }
    };

    let hint_text = if let Some(msg) = &app.status_message {
        format!("{}   |   {}", base_hint, msg)
    } else {
        base_hint.to_string()
    };

    // A clean, reversed-color look is standard for TUI status bars
    let footer = Paragraph::new(hint_text).style(Style::default().fg(Color::Black).bg(Color::Cyan));

    // Render it into that 3rd chunk we created at the top
    f.render_widget(footer, chunks[2]);

    if app.show_help {
        let area = centered_rect(74, 60, f.area());
        f.render_widget(ratatui::widgets::Clear, area);
        let help = Paragraph::new(
            "Quick Help\n\nq: quit\na: add appointment\nd: delete selected appointment\nv: switch day/week view\nt: jump to today\nLeft/Right or h/l: move day\nUp/Down or j/k: move selection\n?: toggle this help\n\nIn Add Mode\nTab: next field\nSpace: toggle checkbox\nUp/Down: adjust time and duration\nEnter: save\nEsc: cancel"
        )
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL).title(" Help "));
        f.render_widget(help, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
