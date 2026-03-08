use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use crate::app::{EditField, RecField, InputMode, App};

pub fn ui(f: &mut Frame, app: &mut App) {
    // 1. Split screen: Header (Top) and Body (Bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header height
            Constraint::Min(0),    // Remaining space
        ])
        .split(f.size());

    // 2. Render Header
    let title = format!(" 📅 Calendar TUI - {} ", app.current_date);
    let header = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    
    f.render_widget(header, chunks[0]);

    // 3. Render Body (The Appointment List)
    let events = app.engine.get_appointments_on_day(app.current_date);
    
    let items: Vec<ListItem> = if events.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "No appointments today.",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        events.iter().map(|evt| {
            // Format: "17:00 - Rust Coding Class"
            let time_str = evt.start.format("%H:%M").to_string();
            let content = format!("{} - {} (ID: {})", time_str, evt.summary, evt.id);
            ListItem::new(Line::from(content))
        }).collect()
    };

    let events_list = List::new(items)
        .block(Block::default().title(" Agenda ").borders(Borders::ALL));

    f.render_widget(events_list, chunks[1]);

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
        let sum_color = if app.active_field == EditField::Summary { Color::Yellow } else { Color::DarkGray };
        let sum_block = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(sum_color))
            .block(Block::default().borders(Borders::ALL).title(" Event Name "));
        f.render_widget(sum_block, popup_chunks[0]);

        // Field 2: Start Time (Spinner)
        let time_color = if app.active_field == EditField::StartTime { Color::Yellow } else { Color::DarkGray };
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
            .block(Block::default().borders(Borders::ALL).title(" Start Time (Up/Down) "));
        f.render_widget(time_block, popup_chunks[1]);

        // Field 3: Duration (Spinner)
        let dur_color = if app.active_field == EditField::Duration { Color::Yellow } else { Color::DarkGray };
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
            .block(Block::default().borders(Borders::ALL).title(" Duration (Up/Down) "));
        f.render_widget(dur_block, popup_chunks[2]);

        // Field 4: Recurring Checkbox
        let rec_color = if app.active_field == EditField::IsRecurring { Color::Yellow } else { Color::DarkGray };
        let check_mark = if app.is_recurring { "[X] Yes" } else { "[ ] No" };
        let rec_text = format!(" {}", check_mark); // Pad with a space

        let rec_block = Paragraph::new(rec_text)
            .style(Style::default().fg(rec_color))
            .block(Block::default().borders(Borders::ALL).title(" Recurring? (Space to toggle) "));
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
            RecField::Mon, RecField::Tue, RecField::Wed, RecField::Thu, 
            RecField::Fri, RecField::Sat, RecField::Sun
        ];
        
        let mut day_spans = Vec::new();
        for i in 0..7 {
            let color = if app.active_rec_field == fields[i] { Color::Yellow } else { Color::DarkGray };
            let check = if app.rec_days[i] { "[X]" } else { "[ ]" };
            day_spans.push(Span::styled(format!("{} {}   ", check, day_names[i]), Style::default().fg(color)));
        }

        let days_block = Paragraph::new(Line::from(day_spans))
            .block(Block::default().borders(Borders::ALL).title(" Select Days "));
        f.render_widget(days_block, popup_chunks[0]);

        // Chunk 2: End Date Toggle
        let toggle_color = if app.active_rec_field == RecField::EndToggle { Color::Yellow } else { Color::DarkGray };
        let check_end = if app.rec_end_date { "[X] Yes" } else { "[ ] No (Runs forever)" };
        let end_toggle_block = Paragraph::new(format!(" {}", check_end))
            .style(Style::default().fg(toggle_color))
            .block(Block::default().borders(Borders::ALL).title(" Has End Date? "));
        f.render_widget(end_toggle_block, popup_chunks[1]);

        // Chunk 3: End Date Weeks (Spinner)
        let weeks_color = if app.active_rec_field == RecField::EndWeeks { Color::Yellow } else { Color::DarkGray };
        let weeks_text = if app.active_rec_field == RecField::EndWeeks {
            format!(" ▲ \n{} Weeks\n ▼ ", app.rec_end_weeks)
        } else {
            format!("\n{} Weeks", app.rec_end_weeks)
        };
        let weeks_block = Paragraph::new(weeks_text)
            .style(Style::default().fg(weeks_color))
            .alignment(ratatui::layout::Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Duration of Recurrence "));
        
        // Only show the spinner clearly if the toggle is actually checked
        if app.rec_end_date {
            f.render_widget(weeks_block, popup_chunks[2]);
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
        .split(popup_layout[1])[1]
}
