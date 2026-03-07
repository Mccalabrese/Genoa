use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use crate::app::InputMode;
use crate::app::App;

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
    if let InputMode::Editing = app.input_mode {
        // Create a clear block to cover the background
        let area = centered_rect(60, 20, f.size());
        
        // We use Clear to wipe out the calendar lines underneath the popup
        f.render_widget(ratatui::widgets::Clear, area); 

        let input_block = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default()
                .borders(Borders::ALL)
                .title(" New Appointment (Press Enter to Save) "));
        
        f.render_widget(input_block, area);
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
