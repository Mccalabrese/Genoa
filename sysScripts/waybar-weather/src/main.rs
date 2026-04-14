//! Waybar Weather Module
//!
//! A focused, asynchronous utility that:
//! 1. Determines the user's geolocation using `geoclue` (via the `where-am-i` utility).
//! 2. Caches location data to minimize GPS polling latency on subsequent runs.
//! 3. Fetches real-time weather and forecast data from OpenWeatherMap.
//! 4. Performs reverse geocoding via OpenStreetMap (Nominatim) to display city/state.
//! 5. Outputs a JSON payload formatted for Waybar custom modules, including Pango markup for tooltips.

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;
use tokio::process::Command;

// Compile regular expressions once for performance optimization.
static LAT_RE: OnceLock<Regex> = OnceLock::new();
static LON_RE: OnceLock<Regex> = OnceLock::new();
static ACC_RE: OnceLock<Regex> = OnceLock::new();
static PANGO_RE: OnceLock<Regex> = OnceLock::new();

// --- Configuration ---

#[derive(Deserialize, Debug)]
struct WaybarWeatherConfig {
    owm_api_key: String,
}
#[derive(Deserialize, Debug)]
struct GlobalConfig {
    waybar_weather: WaybarWeatherConfig,
}
/// Loads configuration from `~/.config/rust-dotfiles/config.toml`.
/// We use `dirs` to ensure cross-platform compatibility (respecting XDG Base Dirs).
fn load_config() -> Result<GlobalConfig> {
    let config_path = dirs::home_dir()
        .context("Could not determine home directory")?
        .join(".config/rust-dotfiles/config.toml");
    let config_str = fs::read_to_string(&config_path).with_context(|| {
        format!(
            "Failed to read config file from path: {}",
            config_path.display()
        )
    })?;
    let config: GlobalConfig = toml::from_str(&config_str)
        .context("Failed to parse config.toml. Check for syntax errors.")?;
    Ok(config)
}
// --- Data Models ---

/// Represents a geolocation coordinate with accuracy metrics.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Location {
    #[serde(rename = "lat")]
    latitude: f64,
    #[serde(rename = "lon")]
    longitude: f64,
    accuracy: f64,
}
#[derive(Serialize)]
struct OwmRequest<'a> {
    #[serde(flatten)]
    location: &'a Location,
    appid: &'a str,
    units: &'a str,
}
// OpenWeatherMap API Response Structures
// I only deserialize the fields we need to keep memory footprint low.
#[derive(Deserialize, Debug, Clone)]
struct Weather {
    id: u32,
    description: String,
}
#[derive(Deserialize, Debug, Clone)]
struct Main {
    temp: f64,
    feels_like: f64,
    humidity: f64,
    pressure: f64,
    temp_min: f64,
    temp_max: f64,
}
#[derive(Deserialize, Debug)]
struct Wind {
    speed: f64,
    deg: Option<f64>,
}
#[derive(Deserialize, Debug)]
struct Sys {
    sunrise: i64,
    sunset: i64,
}
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct CurrentWeather {
    weather: Vec<Weather>,
    main: Main,
    sys: Sys,
    wind: Wind,
    visibility: Option<f64>,
    dt: i64,       // Unix timestamp of data calculation
    timezone: i64, // Shift in seconds from UTC
}
// Nominatim (Reverse Geocoding) Structures
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct NominatimAddress {
    city: Option<String>,
    town: Option<String>,
    village: Option<String>,
    state: Option<String>,
    country: Option<String>,
}
#[derive(Deserialize, Debug)]
struct NominatimResponse {
    address: NominatimAddress,
}
// Forecast Structures
#[derive(Deserialize, Debug)]
struct ForecastItem {
    dt: i64,
    main: Main,
    weather: Vec<Weather>,
    pop: f64, // Probability of Precipitation
}
#[derive(Deserialize, Debug)]
struct Forecast {
    list: Vec<ForecastItem>,
}
// --- Geolocation Logic ---

/// Executes the `where-am-i` system utility to get fresh coordinates.
///
/// This is preferable to using a raw IP-based geolocation API because:
/// 1. It uses Wi-Fi triangulation/GPS (more accurate).
/// 2. It respects system privacy settings via Geoclue.
async fn run_where_am_i() -> Result<Location> {
    let output = Command::new("/usr/lib/geoclue-2.0/demos/where-am-i")
        .output()
        .await
        .context("Failed to run 'where-am-i' command, Is geoclue installed?")?;
    if !output.status.success() {
        anyhow::bail!(
            "'where-am-i' command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse the output using regex to extract coordinates and accuracy
    let lat_re = LAT_RE.get_or_init(|| Regex::new(r"Latitude:\s*(-?\d+\.\d+)").unwrap());
    let lon_re = LON_RE.get_or_init(|| Regex::new(r"Longitude:\s*(-?\d+\.\d+)").unwrap());
    let acc_re = ACC_RE.get_or_init(|| Regex::new(r"Accuracy:\s*(\d+\.?\d*)\s*meters").unwrap());
    let lat_str = lat_re
        .captures(&stdout)
        .context("Failed to parse Latitude")?[1]
        .to_string();
    let lon_str = lon_re
        .captures(&stdout)
        .context("Failed to parse Longitude")?[1]
        .to_string();
    let acc_str = acc_re
        .captures(&stdout)
        .context("Failed to parse Accuracy")?[1]
        .to_string();
    Ok(Location {
        latitude: lat_str.parse()?,
        longitude: lon_str.parse()?,
        accuracy: acc_str.parse()?,
    })
}
// --- Cache Management ---
fn get_cache_path() -> Result<PathBuf> {
    let mut path = dirs::cache_dir().context("Failed to find cache directory")?;
    path.push("weather_location.json");
    Ok(path)
}
fn write_to_cache(location: &Location) -> Result<()> {
    let path = get_cache_path()?;
    let json_data = serde_json::to_string(location)?;
    fs::write(path, json_data)?;
    Ok(())
}
fn read_from_cache() -> Result<Location> {
    let path = get_cache_path()?;
    let json_data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&json_data)?)
}
/// Maps OpenWeatherMap condition IDs to Nerd Font weather icons.
/// Handles day/night variants for Clear and Cloudy conditions.
fn get_weather_icon(condition_id: u32, is_day: bool) -> &'static str {
    match condition_id {
        200..=299 => "󰖓", // Thunderstorm
        300..=399 => "󰖖", // Drizzle
        500..=599 => "󰖖", // Rain
        600..=699 => "󰖘", // Snow
        700..=799 => "󰖑", // Atmosphere
        800 => {
            if is_day {
                "󰖙"
            } else {
                "󰖔"
            }
        } // Clear
        801..=804 => {
            if is_day {
                "󰖐"
            } else {
                "󰖑"
            }
        } // Clouds
        _ => "󰖐",         // Default
    }
}
// --- Network Functions ---
async fn fetch_weather(
    client: &reqwest::Client,
    loc: &Location,
    api_key: &str,
) -> Result<CurrentWeather> {
    let request = OwmRequest {
        location: loc,
        appid: api_key,
        units: "imperial",
    };
    let response = client
        .get("https://api.openweathermap.org/data/2.5/weather")
        .query(&request)
        .send()
        .await?
        .json::<CurrentWeather>()
        .await?;
    Ok(response)
}
/// Performs reverse geocoding to convert coords -> "City, State".
/// Uses OpenStreetMap (Nominatim).
async fn get_city_state(client: &reqwest::Client, loc: &Location) -> Result<(String, String)> {
    //let url = format!(
    //    "https://nominatim.openstreetmap.org/reverse?format=json&lat={}&lon={}&zoom=10",
    //    loc.latitude, loc.longitude
    //);
    let response = client
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(loc)
        .query(&[("format", "json"), ("zoom", "10")])
        .send()
        .await?
        .json::<NominatimResponse>()
        .await?;
    let addr = response.address;
    // Fallback logic: prefer City -> Town -> Village
    let city = addr
        .city
        .or(addr.town)
        .or(addr.village)
        .unwrap_or_else(|| "Unknown City".to_string());
    let state = addr.state.unwrap_or_else(|| "Unknown State".to_string());
    Ok((city, state))
}

async fn fetch_forecast(
    client: &reqwest::Client,
    loc: &Location,
    api_key: &str,
) -> Result<Forecast> {
    let request = OwmRequest {
        location: loc,
        appid: api_key,
        units: "imperial",
    };

    let response = client
        .get("https://api.openweathermap.org/data/2.5/forecast")
        .query(&request)
        .send()
        .await?
        .json::<Forecast>()
        .await?;
    Ok(response)
}

#[tokio::main]
async fn main() -> Result<()> {
    //Initialize Config & Client
    let global_config = load_config()?;
    let api_key = global_config.waybar_weather.owm_api_key;
    // Nominatim uses a strict User-Agent policy to avoid blocking.
    const NOMINATIM_USER_AGENT: &str =
        "WaybarWeatherScript/2.0-owm (Repo: github.com/Mccalabrese/Arch-multi-session-dot-files)";
    let http_client = reqwest::Client::builder()
        .user_agent(NOMINATIM_USER_AGENT)
        .build()?;

    // Obtain Location (with Caching Strategy)
    // Strategy: Try to get a fresh, high-accuracy GPS fix.
    // If that fails (or takes too long/is inaccurate), fall back to the last known good cached location.
    let location = match run_where_am_i().await {
        Ok(fresh) => {
            // Only update cache if the fix is reasonably accurate (< 1500m)
            if fresh.accuracy < 1500.0 {
                let _ = write_to_cache(&fresh);
                fresh
            } else {
                read_from_cache().unwrap_or(fresh)
            }
        }
        Err(e) => {
            eprintln!("'where-am-i' failed: {}. Trying cache...", e);
            read_from_cache().context("Failed to get fresh location AND failed to read cache")?
        }
    };

    // Parallel Network Requests
    // I use tokio::join! to fetch Weather, Geo-data, and Forecast simultaneously
    // to minimize the total runtime of the script.
    let (weather_res, geo_res, forecast_res) = tokio::join!(
        fetch_weather(&http_client, &location, &api_key),
        get_city_state(&http_client, &location),
        fetch_forecast(&http_client, &location, &api_key)
    );

    // Handle Results & Build Output
    let weather_data = match weather_res {
        Ok(data) => data,
        Err(e) => {
            // Output a valid JSON error state for Waybar so the bar doesn't crash
            println!(
                "{}",
                serde_json::json!({
                    "text": "󰖕 API?",
                    "tooltip": format!("Failed to fetch weather: {}", e),
                    "class": "error"
                })
            );
            return Ok(());
        }
    };

    let (city, state) = geo_res.unwrap_or(("Unknown".to_string(), "".to_string()));
    let forecast_data = forecast_res.ok();
    // Calculate Timings (Day/Night)
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let is_day = now >= weather_data.sys.sunrise && now <= weather_data.sys.sunset;
    let icon = get_weather_icon(weather_data.weather[0].id, is_day);

    // Build Tooltip (Pango Markup)
    let mut tooltip_lines = Vec::new();
    tooltip_lines.push(format!(
        "<b>{}, {}</b> (Acc: ~{:.0}m)",
        city, state, location.accuracy
    ));
    tooltip_lines.push(format!(
        "<span size=\"large\">{:.0}°F</span> {} <b>{}</b>",
        weather_data.main.temp, icon, weather_data.weather[0].description
    ));
    tooltip_lines.push(format!(
        "<small>Feels like {:.0}°F</small>",
        weather_data.main.feels_like
    ));
    tooltip_lines.push(format!(
        "Low {:.0}°F / High {:.0}°F",
        weather_data.main.temp_min, weather_data.main.temp_max
    ));
    tooltip_lines.push(String::new()); // Separator
    // Add Wind/Pressure/Vis details                                   //
    let wind_dir = weather_data
        .wind
        .deg
        .map(|d| format!("({:.0}°)", d))
        .unwrap_or_default();
    tooltip_lines.push(format!(
        "󰖝 Wind: {:.1} mph {}",
        weather_data.wind.speed, wind_dir
    ));
    tooltip_lines.push(format!("󰖌 Humidity: {:.0}%", weather_data.main.humidity));
    tooltip_lines.push(format!("󰥡 Pressure: {:.0} hPa", weather_data.main.pressure));
    if let Some(vis) = weather_data.visibility {
        tooltip_lines.push(format!("󰖑 Visibility: {:.1} mi", vis / 1609.34));
    }

    // Append Forecast (Next 3 intervals)
    if let Some(forecast) = forecast_data {
        tooltip_lines.push("\n--- Forecast (3hr) ---".to_string());
        // Calculate timezone offset for correct local time display
        let tz_offset = FixedOffset::east_opt(weather_data.timezone as i32)
            .unwrap_or(FixedOffset::east_opt(0).unwrap());
        for item in forecast.list.iter().take(4) {
            if let Some(dt) = DateTime::from_timestamp(item.dt, 0) {
                let local_time = dt.with_timezone(&tz_offset);
                let time_str = local_time.format("%I%p").to_string();
                let time_clean = time_str.strip_prefix('0').unwrap_or(&time_str);
                //Calculate day/night for forecast icon
                let is_fc_day =
                    item.dt >= weather_data.sys.sunrise && item.dt <= weather_data.sys.sunset;
                let fc_icon = get_weather_icon(item.weather[0].id, is_fc_day);
                let pop_percent = item.pop * 100.0;

                tooltip_lines.push(format!(
                    "{}: {:.0}°F {} (󰖗 {:.0}%)",
                    time_clean, item.main.temp, fc_icon, pop_percent
                ));
            }
        }
    }
    let tooltip = tooltip_lines.join("\n");
    // Write Cleaned Cache (for Lockscreen)
    // I strip Pango tags because Hyprlock/swaylock usually don't support markup in text labels.
    let pango_re =
        PANGO_RE.get_or_init(|| Regex::new(r"</?b>|</b>|</?span.*?>|</?small>").unwrap());
    let cleaned_tooltip = pango_re.replace_all(&tooltip, "").to_string();
    if let Some(cache_dir) = dirs::cache_dir() {
        let _ = fs::write(cache_dir.join(".weather_cache"), cleaned_tooltip);
    }
    // Final Output
    let output_json = serde_json::json!({
        "text": format!("{:.0}°F {}", weather_data.main.temp, icon),
        "tooltip": tooltip,
        "class": "weather"
    });

    println!("{}", output_json);

    Ok(())
}
