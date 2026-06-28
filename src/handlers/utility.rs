// =============================================================================
// handlers/utility.rs — Utility commands (price, time, weather, stats)
// =============================================================================

use anyhow::Result;
use std::time::Instant;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::lib::http;

/// Track bot start time for uptime stats.
pub static START_TIME: once_cell::sync::Lazy<Instant> = once_cell::sync::Lazy::new(Instant::now);

/// Static counter for messages processed.
pub static MESSAGES_PROCESSED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Increment the message counter. Called from the main message handler.
pub fn increment_message_count() {
    MESSAGES_PROCESSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

// -----------------------------------------------------------------------------
// !price — Bitcoin price in USD
// -----------------------------------------------------------------------------

pub async fn price_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let url = "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd&include_24hr_change=true";
    let data = match http::fetch_json(url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("CoinGecko fetch failed: {}", e);
            // Fallback to simple price
            let fallback = http::fetch_json(
                "https://api.coinbase.com/v2/prices/spot?currency=USD",
            ).await;
            match fallback {
                Ok(f) => {
                    let price = &f["data"]["amount"];
                    if let Some(p) = price.as_str() {
                        msg.reply(&format!("₿ Bitcoin: ${}", p)).await?;
                        return Ok(());
                    }
                    msg.reply("⚠️ Could not fetch Bitcoin price right now.").await?;
                    return Ok(());
                }
                Err(_) => {
                    msg.reply("⚠️ Could not fetch Bitcoin price right now.").await?;
                    return Ok(());
                }
            }
        }
    };

    let price = data["bitcoin"]["usd"]
        .as_f64()
        .map(|p| format!("${}", format_number(p)))
        .unwrap_or_else(|| "unavailable".to_string());

    let change = data["bitcoin"]["usd_24h_change"]
        .as_f64()
        .map(|c| {
            let arrow = if c >= 0.0 { "📈" } else { "📉" };
            format!(" {} ({:+.1}% 24h)", arrow, c)
        })
        .unwrap_or_default();

    msg.reply(&format!("₿ Bitcoin: {}{}", price, change)).await?;
    Ok(())
}

/// Format a number with thousands separators (e.g. 67543 → "67,543").
fn format_number(n: f64) -> String {
    let s = format!("{:.0}", n);
    // Insert commas manually (no dependency on locale).
    let bytes = s.as_bytes();
    let mut result = String::new();
    let len = bytes.len();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

// -----------------------------------------------------------------------------
// !time [timezone] — Current time
// -----------------------------------------------------------------------------

pub async fn time_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let tz = args.trim();

    let response = if tz.is_empty() {
        // Show UTC
        let now = chrono::Utc::now();
        format!(
            "🕐 {} UTC",
            now.format("%Y-%m-%d %H:%M:%S")
        )
    } else {
        // Try to parse the timezone
        match tz.parse::<chrono_tz::Tz>() {
            Ok(tz_id) => {
                let now = chrono::Utc::now().with_timezone(&tz_id);
                format!(
                    "🕐 {} ({})",
                    now.format("%Y-%m-%d %H:%M:%S"),
                    tz
                )
            }
            Err(_) => {
                // Unknown timezone — list some common ones
                format!(
                    "⚠️ Unknown timezone \"{}\". Try: UTC, US/Eastern, US/Central, US/Pacific, Europe/London, Asia/Tokyo\nFull list: https://en.wikipedia.org/wiki/List_of_tz_database_time_zones",
                    tz
                )
            }
        }
    };

    msg.reply(&response).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !roll [NdS] — Dice roller
// -----------------------------------------------------------------------------

pub async fn roll_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    use rand::Rng;

    let args = args.trim();

    let (count, sides) = if args.is_empty() {
        // Default: 1d6
        (1u32, 6u32)
    } else if let Some((n, s)) = parse_dice_spec(args) {
        (n, s)
    } else {
        // Try parsing as a single number (e.g., "!roll 20" → 1d20)
        match args.parse::<u32>() {
            Ok(s) => (1, s),
            Err(_) => {
                msg.reply("Usage: !roll [NdS] — e.g. !roll, !roll 20, !roll 3d6").await?;
                return Ok(());
            }
        }
    };

    if count == 0 || count > 100 {
        msg.reply("⚠️ Roll between 1 and 100 dice.").await?;
        return Ok(());
    }
    if sides < 2 || sides > 1000 {
        msg.reply("⚠️ Dice must have 2-1000 sides.").await?;
        return Ok(());
    }

    let (rolls, total) = {
        let mut rng = rand::thread_rng();
        let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
        let total: u32 = rolls.iter().sum();
        (rolls, total)
    };

    let response = if count == 1 {
        format!("🎲 {} (d{})", total, sides)
    } else {
        let rolls_str = rolls
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(" + ");
        format!("🎲 [{}] = {} ({}d{})", rolls_str, total, count, sides)
    };

    msg.reply(&response).await?;
    Ok(())
}

/// Parse a dice spec like "3d6" or "2d20" → (count, sides).
fn parse_dice_spec(spec: &str) -> Option<(u32, u32)> {
    let spec = spec.to_lowercase();
    let parts: Vec<&str> = spec.split('d').collect();
    if parts.len() != 2 {
        return None;
    }
    let count = parts[0].parse::<u32>().ok()?;
    let sides = parts[1].parse::<u32>().ok()?;
    Some((count, sides))
}

// -----------------------------------------------------------------------------
// !stats — Bot statistics
// -----------------------------------------------------------------------------

pub async fn stats_command(ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let uptime_secs = START_TIME.elapsed().as_secs();
    let uptime = format_uptime(uptime_secs);
    let messages = MESSAGES_PROCESSED.load(std::sync::atomic::Ordering::Relaxed);
    let npub = ctx.bot.npub();
    let tracked = ctx.rate_limiter.tracked_users().await;

    let response = format!(
        "📊 **Bot Stats**\n\
         Uptime: {}\n\
         Messages processed: {}\n\
         Tracked users (rate limiter): {}\n\
         Version: v{}\n\
         npub: {}",
        uptime,
        messages,
        tracked,
        env!("CARGO_PKG_VERSION"),
        npub,
    );

    msg.reply(&response).await?;
    Ok(())
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, s)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

// -----------------------------------------------------------------------------
// !weather <zipcode> — Weather via Open-Meteo
// -----------------------------------------------------------------------------

pub async fn weather_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let zipcode = args.trim();

    if zipcode.is_empty() {
        msg.reply("Usage: !weather <zipcode>\nExample: !weather 10001").await?;
        return Ok(());
    }

    // Step 1: Geocode the zipcode → lat/lon
    let geocode_url = format!(
        "https://nominatim.openstreetmap.org/search?postalcode={}&format=json&limit=1&countrycodes=us",
        zipcode
    );

    let geo_data = match http::fetch_json(&geocode_url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Geocode failed: {}", e);
            msg.reply("⚠️ Could not look up that zipcode.").await?;
            return Ok(());
        }
    };

    let geo_arr = geo_data.as_array();
    let first = match geo_arr.and_then(|a| a.first()) {
        Some(v) => v,
        None => {
            msg.reply(&format!("⚠️ No location found for zipcode {}.", zipcode)).await?;
            return Ok(());
        }
    };

    let lat = first["lat"].as_str().or_else(|| first["lat"].as_f64().map(|_| "")).and_then(|_| first["lat"].as_str()).unwrap_or("0");
    let lon = first["lon"].as_str().unwrap_or("0");

    // Handle both string and numeric lat/lon from Nominatim
    let lat = if lat.is_empty() {
        first["lat"].as_f64().map(|f| f.to_string()).unwrap_or_else(|| "0".into())
    } else {
        lat.to_string()
    };
    let lon = if lon.is_empty() {
        first["lon"].as_f64().map(|f| f.to_string()).unwrap_or_else(|| "0".into())
    } else {
        lon.to_string()
    };

    let place_name = first["display_name"]
        .as_str()
        .unwrap_or("Unknown location")
        .split(',')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();

    // Step 2: Fetch weather from Open-Meteo
    let weather_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m&temperature_unit=fahrenheit&timezone=auto",
        lat, lon
    );

    let weather = match http::fetch_json(&weather_url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Weather fetch failed: {}", e);
            msg.reply("⚠️ Could not fetch weather data.").await?;
            return Ok(());
        }
    };

    let current = &weather["current"];
    let temp = current["temperature_2m"].as_f64();
    let feels = current["apparent_temperature_2m"].as_f64();
    let humidity = current["relative_humidity_2m"].as_f64();
    let wind = current["wind_speed_10m"].as_f64();
    let code = current["weather_code"].as_i64();

    let desc = weather_code_to_emoji(code);

    let mut response = format!("🌤️ **{} (ZIP {})**\n", place_name, zipcode);
    if let Some(t) = temp {
        response.push_str(&format!("Temperature: {:.0}°F", t));
        if let Some(f) = feels {
            if (f - t).abs() >= 3.0 {
                response.push_str(&format!(" (feels like {:.0}°F)", f));
            }
        }
        response.push('\n');
    }
    if let Some(h) = humidity {
        response.push_str(&format!("Humidity: {:.0}%\n", h));
    }
    if let Some(w) = wind {
        response.push_str(&format!("Wind: {:.1} mph\n", w));
    }
    response.push_str(&format!("Conditions: {}", desc));

    msg.reply(response.trim()).await?;
    Ok(())
}

/// Convert WMO weather code to description + emoji.
fn weather_code_to_emoji(code: Option<i64>) -> String {
    match code {
        Some(0) => "☀️ Clear sky".to_string(),
        Some(1) => "🌤️ Mainly clear".to_string(),
        Some(2) => "⛅ Partly cloudy".to_string(),
        Some(3) => "☁️ Overcast".to_string(),
        Some(45 | 48) => "🌫️ Fog".to_string(),
        Some(51 | 53 | 55) => "🌦️ Drizzle".to_string(),
        Some(56 | 57) => "🌧️ Freezing drizzle".to_string(),
        Some(61 | 63 | 65) => "🌧️ Rain".to_string(),
        Some(66 | 67) => "🌧️ Freezing rain".to_string(),
        Some(71 | 73 | 75) => "❄️ Snow".to_string(),
        Some(77) => "🌨️ Snow grains".to_string(),
        Some(80 | 81 | 82) => "🌧️ Rain showers".to_string(),
        Some(85 | 86) => "🌨️ Snow showers".to_string(),
        Some(95) => "⛈️ Thunderstorm".to_string(),
        Some(96 | 99) => "⛈️ Thunderstorm with hail".to_string(),
        _ => "❓ Unknown".to_string(),
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dice_spec() {
        assert_eq!(parse_dice_spec("3d6"), Some((3, 6)));
        assert_eq!(parse_dice_spec("1d20"), Some((1, 20)));
        assert_eq!(parse_dice_spec("2d100"), Some((2, 100)));
        assert_eq!(parse_dice_spec("0d6"), Some((0, 6)));
        assert_eq!(parse_dice_spec("abc"), None);
        assert_eq!(parse_dice_spec("3x6"), None);
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(90), "1m 30s");
        assert_eq!(format_uptime(3661), "1h 1m 1s");
        assert_eq!(format_uptime(90061), "1d 1h 1m");
    }

    #[test]
    fn test_weather_code() {
        assert_eq!(weather_code_to_emoji(Some(0)), "☀️ Clear sky");
        assert_eq!(weather_code_to_emoji(Some(3)), "☁️ Overcast");
        assert_eq!(weather_code_to_emoji(Some(95)), "⛈️ Thunderstorm");
        assert_eq!(weather_code_to_emoji(None), "❓ Unknown");
    }
}
