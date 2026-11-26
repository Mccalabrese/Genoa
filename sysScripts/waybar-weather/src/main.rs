//! Waybar Weather Module – Daemon version
//!
//! Runs continuously, prints JSON whenever weather changes.
//! `--prompt` updates the override file and exits; the daemon notices and updates.

use futures_util::StreamExt;
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};
use tokio::time::sleep;
use zbus::{Connection, OwnedObjectPath, Proxy};

const WAYBAR_CACHE_FILE: &str = ".weather_cache.json";
const LOCKSCREEN_CACHE_FILE: &str = ".weather_cache";
const WAYBAR_OVERRIDE_FILE: &str = ".weather_override";

fn log_debug(msg: &str) {
    if let Some(mut path) = get_home_dir() {
        path.push(".cache/weather_debug.log");
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let pid = std::process::id();
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let _ = writeln!(file, "[{now}] [PID {pid}] {msg}");
        }
    }
}

fn get_home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Celsius,
    Fahrenheit,
}

impl Unit {
    fn from_arg(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "c" | "celsius" => Some(Self::Celsius),
            "f" | "fahrenheit" => Some(Self::Fahrenheit),
            _ => None,
        }
    }
    fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
        }
    }
}

fn parse_unit(args: &[String]) -> Option<Unit> {
    args.windows(2).find_map(|pair| match pair[0].as_str() {
        "--unit" | "-u" => Unit::from_arg(pair[1].as_str()),
        _ => None,
    })
}

fn format_temperature(temperature: f64, unit: Unit) -> String {
    let converted = match unit {
        Unit::Celsius => temperature,
        Unit::Fahrenheit => (temperature * 9.0 / 5.0) + 32.0,
    };
    format!("{converted:.1}{}", unit.symbol())
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct WaybarJSON {
    text: String,
    tooltip: String,
    class: String,
}

#[derive(serde::Deserialize)]
struct WeatherResponse {
    current: CurrentWeather,
    daily: DailyForecast,
}

#[derive(serde::Deserialize)]
struct CurrentWeather {
    temperature_2m: f64,
    apparent_temperature: Option<f64>,
    weather_code: i64,
    relative_humidity_2m: Option<f64>,
    wind_speed_10m: Option<f64>,
    pressure_msl: Option<f64>,
    precipitation_probability: Option<f64>,
}

#[derive(serde::Deserialize)]
struct DailyForecast {
    time: Vec<String>,
    weather_code: Vec<i64>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    precipitation_probability_max: Vec<f64>,
}

fn error_payload(message: &str) -> WaybarJSON {
    WaybarJSON {
        text: "???".to_string(),
        tooltip: message.to_string(),
        class: "error".to_string(),
    }
}

fn weather_override_path() -> Option<PathBuf> {
    get_home_dir().map(|p| p.join(".config/waybar").join(WAYBAR_OVERRIDE_FILE))
}

fn write_weather_cache(payload: &WaybarJSON) {
    if let Some(home) = get_home_dir() {
        if let Ok(json) = serde_json::to_string(payload) {
            let _ = fs::write(home.join(format!(".cache/{WAYBAR_CACHE_FILE}")), json);
        }
        let _ = fs::write(
            home.join(format!(".cache/{LOCKSCREEN_CACHE_FILE}")),
            &payload.tooltip,
        );
    }
}

fn read_cached_weather() -> Option<WaybarJSON> {
    let path = get_home_dir()?.join(format!(".cache/{WAYBAR_CACHE_FILE}"));
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn read_weather_override() -> Option<String> {
    let value = fs::read_to_string(weather_override_path()?)
        .ok()?
        .trim()
        .to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn write_weather_override(zip: &str) -> std::io::Result<()> {
    let path = weather_override_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{zip}\n"))
}

fn clear_weather_override() -> std::io::Result<()> {
    let path = weather_override_path().ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[derive(serde::Deserialize)]
struct NominatimResponse {
    address: NominatimAddress,
}

#[derive(serde::Deserialize)]
struct NominatimAddress {
    city: Option<String>,
    town: Option<String>,
    village: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Location {
    lat: f64,
    lon: f64,
}

async fn get_city_state(
    client: &reqwest::Client,
    lat: f64,
    lon: f64,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let response = client
        .get("https://nominatim.openstreetmap.org/reverse")
        .query(&[
            ("lat", lat.to_string()),
            ("lon", lon.to_string()),
            ("format", "json".to_string()),
            ("zoom", "10".to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<NominatimResponse>()
        .await?;

    let addr = response.address;
    let city = addr
        .city
        .or(addr.town)
        .or(addr.village)
        .unwrap_or_else(|| "Unknown City".to_string());
    Ok((city, addr.state.unwrap_or_default()))
}

async fn get_location(
    connection: &Connection,
    path: OwnedObjectPath,
) -> Result<Location, Box<dyn std::error::Error>> {
    let location = Proxy::new(
        connection,
        "org.freedesktop.GeoClue2",
        path,
        "org.freedesktop.GeoClue2.Location",
    )
    .await?;

    Ok(Location {
        lat: location.get_property("Latitude").await?,
        lon: location.get_property("Longitude").await?,
    })
}

async fn run_geoclue() -> Result<Location, Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    let manager = Proxy::new(
        &connection,
        "org.freedesktop.GeoClue2",
        "/org/freedesktop/GeoClue2/Manager",
        "org.freedesktop.GeoClue2.Manager",
    )
    .await?;

    let client_path: OwnedObjectPath = manager.call("GetClient", &()).await?;
    let client = Proxy::new(
        &connection,
        "org.freedesktop.GeoClue2",
        client_path,
        "org.freedesktop.GeoClue2.Client",
    )
    .await?;

    client
        .set_property("DesktopId", "gnome-datetime-panel")
        .await?;
    client.set_property("RequestedAccuracyLevel", 6u32).await?;

    let mut updates = client.receive_signal("LocationUpdated").await?;
    client.call::<_, _, ()>("Start", &()).await?;

    let location_path: OwnedObjectPath = client.get_property("Location").await?;
    if location_path.as_str() != "/" {
        get_location(&connection, location_path).await
    } else {
        match tokio::time::timeout(Duration::from_secs(20), updates.next()).await {
            Ok(Some(signal)) => {
                let (_, new): (OwnedObjectPath, OwnedObjectPath) = signal.body().deserialize()?;
                get_location(&connection, new).await
            }
            _ => Err("GeoClue timeout or stream ended".into()),
        }
    }
}

async fn fetch_weather(
    client: &reqwest::Client,
    lat: f64,
    lon: f64,
) -> Result<WeatherResponse, Box<dyn std::error::Error>> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,apparent_temperature,weather_code,relative_humidity_2m,wind_speed_10m,pressure_msl,precipitation_probability&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&forecast_days=3&timezone=auto"
    );
    Ok(client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn build_weather_payload(city: String, weather: WeatherResponse, unit: Unit) -> WaybarJSON {
    let temp = format_temperature(weather.current.temperature_2m, unit);
    let condition = weather_code_to_string(weather.current.weather_code);
    let icon = weather_code_to_icon(weather.current.weather_code);

    let feels_like = weather
        .current
        .apparent_temperature
        .map(|v| format_temperature(v, unit))
        .unwrap_or_else(|| temp.clone());
    let humidity = weather
        .current
        .relative_humidity_2m
        .map(|v| format!("{v:.0}%"))
        .unwrap_or_else(|| "n/a".into());
    let wind = weather
        .current
        .wind_speed_10m
        .map(|v| format!("{v:.0} km/h"))
        .unwrap_or_else(|| "n/a".into());
    let pressure = weather
        .current
        .pressure_msl
        .map(|v| format!("{v:.0} hPa"))
        .unwrap_or_else(|| "n/a".into());
    let precip = weather
        .current
        .precipitation_probability
        .map(|v| format!("{v:.0}%"))
        .unwrap_or_else(|| "n/a".into());

    let mut tooltip_lines = vec![
        format!("Location: {city}"),
        format!("Current: {condition} • {temp}"),
        format!("Feels like {feels_like} • Humidity {humidity}"),
        format!("Wind: {wind} • Pressure: {pressure}"),
        format!("Precip chance: {precip}"),
    ];

    if !weather.daily.time.is_empty() {
        let fmt_temp = |v: Option<&f64>| {
            v.map(|&t| format_temperature(t, unit))
                .unwrap_or_else(|| "?".into())
        };
        let high = fmt_temp(weather.daily.temperature_2m_max.first());
        let low = fmt_temp(weather.daily.temperature_2m_min.first());
        tooltip_lines.push(format!("High/Low: {high} / {low}"));

        for i in 0..weather.daily.time.len().min(3) {
            let day = &weather.daily.time[i];
            let code = weather
                .daily
                .weather_code
                .get(i)
                .copied()
                .unwrap_or_default();
            let d_high = fmt_temp(weather.daily.temperature_2m_max.get(i));
            let d_low = fmt_temp(weather.daily.temperature_2m_min.get(i));
            let pop = weather
                .daily
                .precipitation_probability_max
                .get(i)
                .map(|v| format!("{v:.0}%"))
                .unwrap_or_else(|| "n/a".into());

            tooltip_lines.push(format!(
                "{day}: {} {} • Hi {d_high} • Lo {d_low} • Rain {pop}",
                weather_code_to_icon(code),
                weather_code_to_string(code)
            ));
        }
    }

    WaybarJSON {
        text: format!("{icon} {temp}"),
        tooltip: tooltip_lines.join("\n"),
        class: "weather".into(),
    }
}

fn weather_code_to_string(code: i64) -> &'static str {
    match code {
        0 => "Clear Sky",
        1 => "Mainly Clear",
        2 => "Partly Cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 | 56 | 57 => "Drizzle",
        61 | 63 | 65 | 66 | 67 => "Rain",
        71 | 73 | 75 | 77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow Showers",
        95 | 96 | 99 => "Thunderstorm",
        _ => "Unknown",
    }
}

fn weather_code_to_icon(code: i64) -> &'static str {
    match code {
        0 | 1 => "☀️",
        2 => "⛅",
        3 => "☁️",
        45 | 48 => "🌁",
        51 | 53 | 55 => "🌦️",
        56 | 57 => "🥶",
        61 | 63 | 65 => "🌧️",
        66 | 67 => "🧊",
        80..=82 => "☔",
        71 | 73 | 75 | 77 => "❄️",
        85 | 86 => "🌨️",
        95 | 96 | 99 => "⛈️",
        _ => "❓",
    }
}

fn prompt_for_location() -> Option<String> {
    let output = Command::new("rofi")
        .args([
            "-dmenu",
            "-p",
            "Enter Zip Code, or type 'auto' to use GPS",
            "-mesg",
            "Leave it blank to return to auto scanning.",
            "-lines",
            "0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?
        .wait_with_output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

async fn zip_to_coords(
    client: &reqwest::Client,
    zip: &str,
) -> Result<Location, Box<dyn std::error::Error>> {
    let body: serde_json::Value = client
        .get("https://nominatim.openstreetmap.org/search")
        .query(&[("q", zip), ("format", "json"), ("limit", "1")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(first) = body.as_array().and_then(|arr| arr.first()) {
        Ok(Location {
            lat: first["lat"].as_str().unwrap_or("0").parse()?,
            lon: first["lon"].as_str().unwrap_or("0").parse()?,
        })
    } else {
        Err(format!("No results for '{zip}'").into())
    }
}

async fn resolve_weather(http_client: &reqwest::Client, unit: Unit) -> Result<WaybarJSON, String> {
    let mut manual_zip = None;

    let location = if let Some(zip) = read_weather_override() {
        match zip_to_coords(http_client, &zip).await {
            Ok(loc) => {
                manual_zip = Some(zip);
                loc
            }
            Err(e) => return Err(format!("Resolving ZIP: {e}")),
        }
    } else {
        run_geoclue().await.map_err(|e| format!("GeoClue: {e}"))?
    };

    let (geo_res, weather_res) = if let Some(zip) = manual_zip {
        (
            Ok((zip, String::new())),
            fetch_weather(http_client, location.lat, location.lon).await,
        )
    } else {
        tokio::join!(
            get_city_state(http_client, location.lat, location.lon),
            fetch_weather(http_client, location.lat, location.lon)
        )
    };

    let (city, state) = geo_res.unwrap_or(("Unknown".into(), String::new()));
    let city_label = if state.is_empty() {
        city
    } else {
        format!("{city}, {state}")
    };

    match weather_res {
        Ok(data) => Ok(build_weather_payload(city_label, data, unit)),
        Err(e) => Err(format!("Weather fetch failed: {e}")),
    }
}

fn print_payload(payload: &WaybarJSON) {
    if let Ok(json) = serde_json::to_string(payload) {
        println!("{json}");
        let _ = std::io::stdout().flush();
    }
}

async fn run_daemon(http_client: reqwest::Client, unit: Unit) {
    // Get initial mtime before loop starts so we don't accidentally trigger a double-fetch on launch.
    let get_mtime = || weather_override_path().and_then(|p| fs::metadata(p).ok()?.modified().ok());
    let mut last_override_mtime = get_mtime();

    loop {
        match resolve_weather(&http_client, unit).await {
            Ok(payload) => {
                write_weather_cache(&payload);
                print_payload(&payload);
            }
            Err(e) => {
                log_debug(&format!("Daemon fetch failed: {e}"));
                if let Some(cached) = read_cached_weather() {
                    print_payload(&cached);
                } else {
                    print_payload(&error_payload(&format!("Weather error: {e}")));
                }
            }
        }

        // Poll for override file changes (60 iterations * 5s = 5 minutes).
        // Falls through naturally to run the periodic 5-minute refresh.
        for _ in 0..60 {
            sleep(Duration::from_secs(5)).await;

            let current_mtime = get_mtime();
            if current_mtime != last_override_mtime {
                log_debug("Daemon: Override file changed, fetching immediately.");
                last_override_mtime = current_mtime;
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    let unit = parse_unit(&args).unwrap_or_else(|| {
        env::var("WAYBAR_WEATHER_UNIT")
            .ok()
            .and_then(|v| Unit::from_arg(&v))
            .unwrap_or(Unit::Celsius)
    });

    let http_client = reqwest::Client::builder()
        .user_agent("WaybarWeatherScript/3.0-owm")
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    if args.iter().any(|arg| arg == "--prompt") {
        if let Some(input) = prompt_for_location() {
            if input.trim().is_empty() || input.trim().eq_ignore_ascii_case("auto") {
                let _ = clear_weather_override();
            } else {
                let _ = write_weather_override(&input);
            }
        }
        return;
    }

    log_debug("Starting daemon mode.");
    run_daemon(http_client, unit).await;
}
