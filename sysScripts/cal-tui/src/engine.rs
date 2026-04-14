use crate::model::{Appointment, Recurrence};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct StorageData {
    next_id: u32,
    appointments: HashMap<u32, Appointment>,
}

pub struct CalendarEngine {
    appointments: HashMap<u32, Appointment>,
    next_id: u32,
}

impl CalendarEngine {
    pub fn new() -> Self {
        CalendarEngine {
            appointments: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn add_appointment(&mut self, mut appointment: Appointment) -> u32 {
        let id = self.next_id;
        appointment.id = id;
        self.appointments.insert(id, appointment);
        self.next_id += 1;
        id
    }

    pub fn remove_appointment(&mut self, id: u32) -> bool {
        // Removes the item and returns true if it existed
        self.appointments.remove(&id).is_some()
    }

    pub fn get_appointments_on_day(&self, date: NaiveDate) -> Vec<&Appointment> {
        let mut matches = Vec::new();
        for appointment in self.appointments.values() {
            if self.occurs_on(appointment, date) {
                matches.push(appointment);
            }
        }
        matches.sort_by_key(|a| a.start);
        matches
    }

    pub fn occurs_on(&self, app: &Appointment, target_date: NaiveDate) -> bool {
        let start_date = app.start.date_naive();
        if start_date > target_date {
            return false;
        }
        if app.rule.is_none() {
            return start_date == target_date;
        }

        if app
            .exceptions
            .iter()
            .any(|ex| ex.date_naive() == target_date)
        {
            return false;
        }

        match app.rule.as_ref().unwrap() {
            Recurrence::Daily { until } => {
                if let Some(end_dt) = until
                    && target_date > end_dt.date_naive()
                {
                    return false;
                }
                true
            }
            Recurrence::Weekly { days, until } => {
                if let Some(end_dt) = until
                    && target_date > end_dt.date_naive()
                {
                    return false;
                }

                days.contains(&target_date.weekday())
            }
        }
    }
    pub fn save_to_file(&self, filename: &str) -> std::io::Result<()> {
        let data = StorageData {
            next_id: self.next_id,
            appointments: self.appointments.clone(),
        };
        let json = serde_json::to_string_pretty(&data)?;
        fs::write(filename, json)?;
        Ok(())
    }
    pub fn load_from_file(filename: &str) -> Self {
        if !Path::new(filename).exists() {
            return Self::new();
        }

        let content = match fs::read_to_string(filename) {
            Ok(c) => c,
            Err(_) => return Self::new(),
        };

        match serde_json::from_str::<StorageData>(&content) {
            Ok(data) => Self {
                appointments: data.appointments,
                next_id: data.next_id,
            },
            Err(e) => {
                eprintln!("Failed to parse calendar data: {}", e);
                Self::new()
            }
        }
    }
}
