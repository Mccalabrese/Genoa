use chrono::{DateTime, Duration, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Appointment {
    pub id: u32,
    pub summary: String,
    pub start: DateTime<Utc>,
    pub duration: Duration,
    pub rule: Option<Recurrence>,
    #[serde(default)]
    pub exceptions: Vec<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Recurrence {
    Daily {
        until: Option<DateTime<Utc>>,
    },
    Weekly {
        days: Vec<Weekday>,
        until: Option<DateTime<Utc>>,
    },
}
