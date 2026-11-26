//! The intentionally small, local-only lock-screen presentation.
//!
//! This module never runs a command or reads executable configuration. The
//! wallpaper is compiled into the binary; weather is a bounded, sanitized read
//! of Genoa's existing user-owned cache.

use std::{fs::File, io::Read};

use chrono::Local;
use font8x8::{BASIC_FONTS, UnicodeFonts};
use image::ImageFormat;
use serde::Deserialize;
use uzers::{get_current_uid, get_user_by_uid, os::unix::UserExt};

const WALLPAPER: &[u8] = include_bytes!("../../../wallpapers/comp1.jpg");
const WEATHER_CACHE_MAX_BYTES: u64 = 4 * 1024;
const MAX_WEATHER_LINES: usize = 9;
const MAX_WEATHER_LINE_CHARS: usize = 58;

pub struct Appearance {
    wallpaper: Wallpaper,
    weather: WeatherDisplay,
}

struct Wallpaper {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct WeatherCache {
    text: String,
    tooltip: String,
}

#[derive(Clone, Copy)]
enum WeatherIcon {
    Clear,
    Cloudy,
    Fog,
    Rain,
    Snow,
    Storm,
    Unknown,
}

struct WeatherDisplay {
    lines: Vec<String>,
    icon: WeatherIcon,
}

impl Appearance {
    pub fn load() -> Result<Self, String> {
        let image = image::load_from_memory_with_format(WALLPAPER, ImageFormat::Jpeg)
            .map_err(|error| format!("could not decode the embedded lock wallpaper: {error}"))?
            .to_rgba8();
        Ok(Self {
            wallpaper: Wallpaper {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            },
            weather: WeatherDisplay::load(),
        })
    }

    pub fn frame(
        &self,
        width: u32,
        height: u32,
        password_len: usize,
        authenticating: bool,
    ) -> Result<Vec<u8>, String> {
        let pixels = pixel_len(width, height)?;
        let mut frame = vec![0_u8; pixels];
        self.draw_wallpaper(&mut frame, width, height);
        darken(&mut frame, 96);

        let centre_x = i32::try_from(width / 2).map_err(|_| "lock width is too large")?;
        let height_i32 = i32::try_from(height).map_err(|_| "lock height is too large")?;
        let time = Local::now().format("%H:%M").to_string();
        draw_centered_text(
            &mut frame,
            width,
            height,
            &time,
            centre_x,
            height_i32 / 3,
            10,
            [222, 210, 195, 255],
        );

        let box_width = i32::try_from(width.min(520)).map_err(|_| "lock width is too large")?;
        let box_height = 68;
        let box_x = centre_x - box_width / 2;
        let box_y = height_i32 / 2;
        fill_rect(
            &mut frame,
            width,
            height,
            box_x,
            box_y,
            box_width,
            box_height,
            [18, 18, 18, 190],
        );
        stroke_rect(
            &mut frame,
            width,
            height,
            box_x,
            box_y,
            box_width,
            box_height,
            [154, 148, 144, 255],
        );

        let message = if authenticating {
            "AUTHENTICATING"
        } else if password_len == 0 {
            "PASSWORD"
        } else {
            ""
        };
        if message.is_empty() {
            draw_password_dots(
                &mut frame,
                width,
                height,
                centre_x,
                box_y + box_height / 2,
                password_len,
            );
        } else {
            draw_centered_text(
                &mut frame,
                width,
                height,
                message,
                centre_x,
                box_y + 23,
                2,
                [222, 210, 195, 255],
            );
        }

        self.weather.draw(&mut frame, width, height);
        Ok(frame)
    }

    fn draw_wallpaper(&self, frame: &mut [u8], width: u32, height: u32) {
        let source_width = u64::from(self.wallpaper.width);
        let source_height = u64::from(self.wallpaper.height);
        let target_width = u64::from(width);
        let target_height = u64::from(height);
        let (crop_x, crop_y, crop_width, crop_height) =
            if source_width * target_height > source_height * target_width {
                let crop_width = source_height * target_width / target_height;
                (
                    (source_width - crop_width) / 2,
                    0,
                    crop_width,
                    source_height,
                )
            } else {
                let crop_height = source_width * target_height / target_width;
                (
                    0,
                    (source_height - crop_height) / 2,
                    source_width,
                    crop_height,
                )
            };

        for y in 0..height {
            let source_y = crop_y + u64::from(y) * crop_height / target_height;
            for x in 0..width {
                let source_x = crop_x + u64::from(x) * crop_width / target_width;
                let source_offset =
                    usize::try_from((source_y * source_width + source_x) * 4).unwrap_or(0);
                let target_offset =
                    usize::try_from((u64::from(y) * target_width + u64::from(x)) * 4).unwrap_or(0);
                let rgba = &self.wallpaper.rgba[source_offset..source_offset + 4];
                // wl_shm's ARGB8888 is native-endian; on Genoa's supported
                // little-endian targets the backing bytes are BGRA.
                frame[target_offset..target_offset + 4]
                    .copy_from_slice(&[rgba[2], rgba[1], rgba[0], 255]);
            }
        }
    }
}

impl WeatherDisplay {
    fn load() -> Self {
        let Some(user) = get_user_by_uid(get_current_uid()) else {
            return Self::empty();
        };
        let path = user.home_dir().join(".cache/.weather_cache.json");
        let mut contents = String::new();
        let Ok(file) = File::open(path) else {
            return Self::empty();
        };
        if file
            .take(WEATHER_CACHE_MAX_BYTES)
            .read_to_string(&mut contents)
            .is_err()
        {
            return Self::empty();
        }
        let Ok(cache) = serde_json::from_str::<WeatherCache>(&contents) else {
            return Self::empty();
        };
        let mut lines = cache
            .tooltip
            .lines()
            .map(sanitize_weather_line)
            .filter(|line| !line.is_empty())
            .take(MAX_WEATHER_LINES)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            let text = sanitize_weather_line(&cache.text);
            if !text.is_empty() {
                lines.push(text);
            }
        }
        let icon = weather_icon(&cache.tooltip);
        Self { lines, icon }
    }

    const fn empty() -> Self {
        Self {
            lines: Vec::new(),
            icon: WeatherIcon::Unknown,
        }
    }

    fn draw(&self, pixels: &mut [u8], width: u32, height: u32) {
        if self.lines.is_empty() {
            return;
        }
        let line_height = 20_i32;
        // Leave a consistent margin even on small displays, while giving the
        // detailed forecast enough room on normal laptop and desktop outputs.
        let available_width = width.saturating_sub(56);
        let panel_width = i32::try_from(available_width.min(1_160)).unwrap_or(1_160);
        let panel_height = 24_i32.saturating_add(
            i32::try_from(self.lines.len())
                .unwrap_or(i32::MAX)
                .saturating_mul(line_height),
        );
        let panel_x = 28;
        let panel_y = i32::try_from(height)
            .unwrap_or(i32::MAX)
            .saturating_sub(panel_height + 28)
            .max(20);
        fill_rect(
            pixels,
            width,
            height,
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            [12, 12, 12, 170],
        );
        stroke_rect(
            pixels,
            width,
            height,
            panel_x,
            panel_y,
            panel_width,
            panel_height,
            [154, 148, 144, 230],
        );
        draw_weather_icon(pixels, width, height, self.icon, panel_x + 28, panel_y + 28);
        let text_x = panel_x + 62;
        for (index, line) in self.lines.iter().enumerate() {
            draw_text(
                pixels,
                width,
                height,
                line,
                text_x,
                panel_y + 12 + i32::try_from(index).unwrap_or(0) * line_height,
                2,
                [235, 235, 235, 255],
            );
        }
    }
}

fn sanitize_weather_line(line: &str) -> String {
    line.chars()
        .flat_map(|character| match character {
            '•' => " / ".chars().collect::<Vec<_>>(),
            '°' => " deg".chars().collect::<Vec<_>>(),
            character if character.is_ascii_graphic() || character == ' ' => vec![character],
            character if character.is_whitespace() => vec![' '],
            _ => Vec::new(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_WEATHER_LINE_CHARS)
        .collect()
}

fn weather_icon(tooltip: &str) -> WeatherIcon {
    if tooltip.contains("Thunderstorm") {
        WeatherIcon::Storm
    } else if tooltip.contains("Snow") {
        WeatherIcon::Snow
    } else if tooltip.contains("Rain") || tooltip.contains("Drizzle") || tooltip.contains("Showers")
    {
        WeatherIcon::Rain
    } else if tooltip.contains("Fog") {
        WeatherIcon::Fog
    } else if tooltip.contains("Cloudy") || tooltip.contains("Overcast") {
        WeatherIcon::Cloudy
    } else if tooltip.contains("Clear") {
        WeatherIcon::Clear
    } else {
        WeatherIcon::Unknown
    }
}

fn draw_weather_icon(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    icon: WeatherIcon,
    centre_x: i32,
    centre_y: i32,
) {
    const SKY: [u8; 4] = [235, 210, 100, 255];
    const CLOUD: [u8; 4] = [205, 215, 225, 255];
    const RAIN: [u8; 4] = [105, 175, 240, 255];
    const WHITE: [u8; 4] = [240, 240, 240, 255];

    match icon {
        WeatherIcon::Clear => {
            fill_circle(pixels, width, height, centre_x, centre_y, 10, SKY);
            for (x, y) in [(0, -16), (0, 16), (-16, 0), (16, 0)] {
                fill_rect(
                    pixels,
                    width,
                    height,
                    centre_x + x - 1,
                    centre_y + y - 4,
                    3,
                    8,
                    SKY,
                );
            }
        }
        WeatherIcon::Cloudy | WeatherIcon::Fog => {
            draw_cloud(pixels, width, height, centre_x, centre_y, CLOUD);
            if matches!(icon, WeatherIcon::Fog) {
                for offset in [-10, 0, 10] {
                    fill_rect(
                        pixels,
                        width,
                        height,
                        centre_x - 18,
                        centre_y + offset,
                        36,
                        2,
                        WHITE,
                    );
                }
            }
        }
        WeatherIcon::Rain => {
            draw_cloud(pixels, width, height, centre_x, centre_y - 5, CLOUD);
            for offset in [-10, 0, 10] {
                fill_rect(
                    pixels,
                    width,
                    height,
                    centre_x + offset,
                    centre_y + 10,
                    2,
                    8,
                    RAIN,
                );
            }
        }
        WeatherIcon::Snow => {
            draw_cloud(pixels, width, height, centre_x, centre_y - 5, CLOUD);
            for offset in [-10, 0, 10] {
                fill_rect(
                    pixels,
                    width,
                    height,
                    centre_x + offset - 3,
                    centre_y + 13,
                    7,
                    2,
                    WHITE,
                );
                fill_rect(
                    pixels,
                    width,
                    height,
                    centre_x + offset,
                    centre_y + 10,
                    2,
                    7,
                    WHITE,
                );
            }
        }
        WeatherIcon::Storm => {
            draw_cloud(pixels, width, height, centre_x, centre_y - 6, CLOUD);
            fill_rect(pixels, width, height, centre_x - 1, centre_y + 5, 6, 3, SKY);
            fill_rect(pixels, width, height, centre_x - 4, centre_y + 8, 6, 3, SKY);
            fill_rect(
                pixels,
                width,
                height,
                centre_x - 4,
                centre_y + 11,
                3,
                5,
                SKY,
            );
        }
        WeatherIcon::Unknown => fill_circle(pixels, width, height, centre_x, centre_y, 8, WHITE),
    }
}

fn draw_cloud(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    centre_x: i32,
    centre_y: i32,
    color: [u8; 4],
) {
    fill_circle(pixels, width, height, centre_x - 10, centre_y, 8, color);
    fill_circle(pixels, width, height, centre_x, centre_y - 5, 11, color);
    fill_circle(pixels, width, height, centre_x + 11, centre_y, 8, color);
    fill_rect(
        pixels,
        width,
        height,
        centre_x - 18,
        centre_y,
        36,
        10,
        color,
    );
}

fn pixel_len(width: u32, height: u32) -> Result<usize, String> {
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "lock surface dimensions overflow".to_string())
}

fn darken(pixels: &mut [u8], opacity: u8) {
    let remaining = u16::from(255_u8.saturating_sub(opacity));
    let (pixel_rows, _) = pixels.as_chunks_mut::<4>();
    for pixel in pixel_rows {
        for channel in &mut pixel[..3] {
            *channel = u8::try_from(u16::from(*channel) * remaining / 255).unwrap_or(0);
        }
    }
}

// These are deliberately direct drawing primitives. Keeping coordinates and
// color explicit at each call site makes the small renderer auditable.
#[allow(clippy::too_many_arguments)]
fn draw_centered_text(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    text: &str,
    centre_x: i32,
    y: i32,
    scale: i32,
    color: [u8; 4],
) {
    let glyph_width = 8_i32.saturating_mul(scale);
    let text_width = i32::try_from(text.chars().count())
        .unwrap_or(i32::MAX)
        .saturating_mul(glyph_width);
    draw_text(
        pixels,
        width,
        height,
        text,
        centre_x.saturating_sub(text_width / 2),
        y,
        scale,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    text: &str,
    x: i32,
    y: i32,
    scale: i32,
    color: [u8; 4],
) {
    for (index, character) in text.chars().enumerate() {
        let Some(glyph) = BASIC_FONTS.get(character) else {
            continue;
        };
        let glyph_x = x.saturating_add(
            i32::try_from(index)
                .unwrap_or(i32::MAX)
                .saturating_mul(8 * scale),
        );
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..8_u8 {
                if bits & (1_u8 << column) != 0 {
                    fill_rect(
                        pixels,
                        width,
                        height,
                        glyph_x + i32::from(column) * scale,
                        y + i32::try_from(row).unwrap_or(0) * scale,
                        scale,
                        scale,
                        color,
                    );
                }
            }
        }
    }
}

fn draw_password_dots(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    centre_x: i32,
    centre_y: i32,
    count: usize,
) {
    let visible = count.min(24);
    let spacing = 18;
    let start = centre_x
        .saturating_sub(i32::try_from(visible.saturating_sub(1)).unwrap_or(0) * spacing / 2);
    for index in 0..visible {
        fill_circle(
            pixels,
            width,
            height,
            start + i32::try_from(index).unwrap_or(0) * spacing,
            centre_y,
            5,
            [222, 210, 195, 255],
        );
    }
    if count > visible {
        fill_circle(
            pixels,
            width,
            height,
            start + i32::try_from(visible).unwrap_or(0) * spacing,
            centre_y,
            3,
            [222, 210, 195, 255],
        );
    }
}

fn fill_circle(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    centre_x: i32,
    centre_y: i32,
    radius: i32,
    color: [u8; 4],
) {
    for y in -radius..=radius {
        for x in -radius..=radius {
            if x * x + y * y <= radius * radius {
                put_pixel(pixels, width, height, centre_x + x, centre_y + y, color);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: [u8; 4],
) {
    fill_rect(pixels, width, height, x, y, rect_width, 2, color);
    fill_rect(
        pixels,
        width,
        height,
        x,
        y + rect_height - 2,
        rect_width,
        2,
        color,
    );
    fill_rect(pixels, width, height, x, y, 2, rect_height, color);
    fill_rect(
        pixels,
        width,
        height,
        x + rect_width - 2,
        y,
        2,
        rect_height,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: [u8; 4],
) {
    for row in 0..rect_height {
        for column in 0..rect_width {
            put_pixel(pixels, width, height, x + column, y + row, color);
        }
    }
}

fn put_pixel(pixels: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: [u8; 4]) {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return;
    };
    if x >= width || y >= height {
        return;
    }
    let offset = usize::try_from((u64::from(y) * u64::from(width) + u64::from(x)) * 4).unwrap_or(0);
    let alpha = u16::from(color[3]);
    for index in 0..3 {
        let blended = (u16::from(color[index]) * alpha
            + u16::from(pixels[offset + index]) * (255 - alpha))
            / 255;
        pixels[offset + index] = u8::try_from(blended).unwrap_or(0);
    }
    pixels[offset + 3] = 255;
}

#[cfg(test)]
mod tests {
    use super::Appearance;

    #[test]
    fn embedded_appearance_renders_an_opaque_frame() -> Result<(), String> {
        let appearance = Appearance::load()?;
        let frame = appearance.frame(64, 64, 3, false)?;

        assert_eq!(frame.len(), 64 * 64 * 4);
        let (pixels, remainder) = frame.as_chunks::<4>();
        assert!(remainder.is_empty());
        assert!(pixels.iter().all(|pixel| pixel[3] == 255));
        Ok(())
    }
}
