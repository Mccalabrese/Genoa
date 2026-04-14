use crate::engine::CalendarEngine;
use chrono::{NaiveDate, Utc};
use directories::ProjectDirs;
use ratatui::widgets::ListState;
use std::fs;

#[derive(PartialEq)]
pub enum InputMode {
    Normal,
    Editing,
    EditingRecurrence,
}

#[derive(PartialEq)]
pub enum EditField {
    Summary,
    StartTime,
    Duration,
    IsRecurring,
}

#[derive(PartialEq)]
pub enum ViewMode {
    Day,
    Week,
}

#[derive(PartialEq)]
pub enum RecField {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
    EndToggle,
    EndWeeks,
}

pub struct App {
    pub engine: CalendarEngine,
    pub current_date: NaiveDate, // The day selected in the UI
    pub should_quit: bool,

    pub list_state: ListState,
    pub view_mode: ViewMode,

    pub input_mode: InputMode,
    pub active_field: EditField,
    pub active_rec_field: RecField,

    pub input_buffer: String,
    pub time_minutes: u32,
    pub duration_minutes: u32,
    pub is_recurring: bool,

    pub rec_days: [bool; 7],
    pub rec_end_date: bool,
    pub rec_end_weeks: u32,

    pub show_help: bool,
    pub status_message: Option<String>,
}

impl App {
    // Get the safe data path (~/.local/share/cal-tui/calendar_data.json)
    fn get_data_path() -> String {
        if let Some(proj_dirs) = ProjectDirs::from("", "", "cal-tui") {
            let dir = proj_dirs.data_dir();
            // If ~/.local/share/cal-tui doesn't exist, create it!
            if !dir.exists() {
                fs::create_dir_all(dir).expect("Failed to create data directory");
            }
            // Return the full path to the json file
            dir.join("calendar_data.json").to_str().unwrap().to_string()
        } else {
            // Fallback just in case they have a weird OS setup
            "calendar_data.json".to_string()
        }
    }
    pub fn new() -> Self {
        let path = Self::get_data_path();
        let engine = CalendarEngine::load_from_file(&path);
        Self {
            engine,
            current_date: Utc::now().date_naive(),
            should_quit: false,
            view_mode: ViewMode::Day,
            list_state: ListState::default(),
            input_mode: InputMode::Normal,
            active_field: EditField::Summary,
            active_rec_field: RecField::Mon, // Start on Monday

            input_buffer: String::new(),
            time_minutes: 720,
            duration_minutes: 60,
            is_recurring: false,

            rec_days: [false; 7],
            rec_end_date: false,
            rec_end_weeks: 16, // Default to a standard 16-week semester
            show_help: false,
            status_message: None,
        }
    }

    // NEW: Centralized save method
    pub fn save(&self) {
        let path = Self::get_data_path();
        if let Err(e) = self.engine.save_to_file(&path) {
            eprintln!("Failed to save calendar data: {}", e);
        }
    }

    pub fn reset_form(&mut self) {
        self.input_buffer.clear();
        self.active_field = EditField::Summary;
        self.active_rec_field = RecField::Mon;
        self.time_minutes = 720;
        self.duration_minutes = 60;
        self.is_recurring = false;
        self.rec_days = [false; 7];
        self.rec_end_date = false;
        self.rec_end_weeks = 16;
    }

    pub fn set_status<S: Into<String>>(&mut self, message: S) {
        self.status_message = Some(message.into());
    }

    pub fn quit(&mut self) {
        self.save();
        self.should_quit = true;
    }
}
