// =============================================================================
// handlers/utility.rs — Utility commands (price, time, weather, stats)
// =============================================================================

use anyhow::Result;
use std::time::Instant;
use vector_sdk::IncomingMessage;

use crate::bot::BotContext;
use crate::lib::http;

// -----------------------------------------------------------------------------
// !delete <msg_id> — Delete a message (bot's own or MANAGE_MESSAGES in v2)
// -----------------------------------------------------------------------------

pub async fn delete_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let msg_id = args.trim();
    if msg_id.is_empty() {
        msg.reply("Usage: !delete <message_id>\n\nDeletes a message. You can only delete your own messages, or messages in communities where you have MANAGE_MESSAGES capability.").await?;
        return Ok(());
    }

    let channel = msg.channel();
    match channel.delete(msg_id).await {
        Ok(()) => { msg.reply("🗑️ Message deleted.").await?; }
        Err(e) => {
            let err = format!("{:?}", e).to_lowercase();
            if err.contains("permission") || err.contains("manage") {
                msg.reply("⚠️ I don't have permission to delete that message.").await?;
            } else {
                msg.reply(&format!("⚠️ Delete failed: {}", e)).await?;
            }
        }
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// !edit <msg_id> <text> — Edit a message (bot's own only)
// -----------------------------------------------------------------------------

pub async fn edit_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    if parts.len() < 2 {
        msg.reply("Usage: !edit <message_id> <new text>").await?;
        return Ok(());
    }
    let msg_id = parts[0].trim();
    let new_text = parts[1].trim();

    if msg_id.is_empty() || new_text.is_empty() {
        msg.reply("Usage: !edit <message_id> <new text>").await?;
        return Ok(());
    }

    let channel = msg.channel();
    match channel.edit(msg_id, new_text).await {
        Ok(()) => { msg.reply("✏️ Message edited.").await?; }
        Err(e) => { msg.reply(&format!("⚠️ Edit failed: {}", e)).await?; }
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// !savefile — Save an attachment to disk
// -----------------------------------------------------------------------------

pub async fn savefile_command(ctx: &BotContext, msg: &IncomingMessage, _args: &str) -> Result<()> {
    if !msg.is_file {
        msg.reply("⚠️ This command works on messages with file attachments. Reply to a file message with !savefile.").await?;
        return Ok(());
    }

    let download_dir = std::path::PathBuf::from("./data/downloads");
    if let Err(e) = std::fs::create_dir_all(&download_dir) {
        msg.reply(&format!("⚠️ Failed to create download dir: {}", e)).await?;
        return Ok(());
    }

    for att in &msg.message.attachments {
        let filename = if att.name.is_empty() {
            format!("{}.{}", att.id, att.extension)
        } else {
            att.name.clone()
        };

        match ctx.bot.save_attachment(att, download_dir.join(&filename)).await {
            Ok(path) => {
                msg.reply(&format!("💾 Saved {} ({} bytes) to {}", filename, att.size, path.display())).await?;
            }
            Err(e) => {
                msg.reply(&format!("⚠️ Failed to save {}: {}", filename, e)).await?;
            }
        }
    }
    Ok(())
}

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
// !remind <time> <message> — Set a reminder (echo for now, persistence later)
// -----------------------------------------------------------------------------

pub async fn remind_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        msg.reply(
            "Usage: !remind <time> <message>\nExamples: !remind 30m Call mom, !remind 2h Check oven, !remind 1d Pay bills"
        ).await?;
        return Ok(());
    }

    match parse_reminder_input(args) {
        Some((desc, message)) => {
            if message.is_empty() {
                msg.reply("⚠️ Please provide a message after the time.\nExample: !remind 30m Call mom").await?;
            } else {
                msg.reply(&format!("⏰ Reminder set: {} (in {})", message, desc)).await?;
            }
        }
        None => {
            msg.reply(
                "⚠️ Could not parse time. Use: 30m, 2h, 1d, 30 minutes, 2 hours, 1 day\nExample: !remind 30m Call mom"
            ).await?;
        }
    }

    Ok(())
}

/// Parse reminder input like "30m Call mom" or "2 hours Check oven".
/// Returns (human-readable duration description, message text).
fn parse_reminder_input(input: &str) -> Option<(String, String)> {
    let input = input.trim();

    // Parse leading number
    let num_end = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    if num_end == 0 {
        return None;
    }
    let num: u64 = input[..num_end].parse().ok()?;
    let mut remaining = &input[num_end..];

    // Skip optional space between number and unit
    remaining = remaining.trim_start();

    // Parse unit (alphabetic chars)
    let unit_end = remaining
        .find(|c: char| !c.is_alphabetic())
        .unwrap_or(remaining.len());
    if unit_end == 0 {
        return None;
    }
    let unit = remaining[..unit_end].to_lowercase();
    remaining = &remaining[unit_end..];

    let desc = match unit.as_str() {
        "m" | "min" | "mins" | "minute" | "minutes" => {
            format!("{} minute{}", num, if num == 1 { "" } else { "s" })
        }
        "h" | "hr" | "hrs" | "hour" | "hours" => {
            format!("{} hour{}", num, if num == 1 { "" } else { "s" })
        }
        "d" | "day" | "days" => {
            format!("{} day{}", num, if num == 1 { "" } else { "s" })
        }
        _ => return None,
    };

    let message = remaining.trim().to_string();
    Some((desc, message))
}

// -----------------------------------------------------------------------------
// !poll <question> | option1 | option2 | ... — Create a poll
// -----------------------------------------------------------------------------

pub async fn poll_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        msg.reply("Usage: !poll <question> | option1 | option2 | ...\nExample: !poll Pizza? | Yes | No | Maybe").await?;
        return Ok(());
    }

    let parts: Vec<&str> = args
        .split('|')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if parts.len() < 3 {
        msg.reply("⚠️ Need a question and at least 2 options.\nExample: !poll Pizza? | Yes | No").await?;
        return Ok(());
    }

    let question = parts[0];
    let options = &parts[1..];

    let mut response = format!("📊 **{}**\n", question);
    for (i, opt) in options.iter().enumerate() {
        response.push_str(&format!("  {}️ {}\n", number_emoji(i + 1), opt));
    }
    response.push_str("\nReact to vote!");

    msg.reply(response.trim()).await?;
    Ok(())
}

/// Convert a number 1-10 to its emoji representation.
fn number_emoji(n: usize) -> &'static str {
    match n {
        1 => "1⃣",
        2 => "2⃣",
        3 => "3⃣",
        4 => "4⃣",
        5 => "5⃣",
        6 => "6⃣",
        7 => "7⃣",
        8 => "8⃣",
        9 => "9⃣",
        10 => "🔟",
        _ => "❓",
    }
}

// -----------------------------------------------------------------------------
// !translate <language> <text> — Translate text via MyMemory API
// -----------------------------------------------------------------------------

pub async fn translate_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let args = args.trim();

    if args.is_empty() {
        msg.reply("Usage: !translate <language> <text>\nExample: !translate es Hello world").await?;
        return Ok(());
    }

    // Split: first word is language code, rest is text
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    if parts.len() < 2 {
        msg.reply("⚠️ Provide a language code and text.\nExample: !translate es Hello world").await?;
        return Ok(());
    }

    let lang = parts[0].trim();
    let text = parts[1].trim();

    if text.is_empty() {
        msg.reply("⚠️ Provide text to translate.\nExample: !translate es Hello world").await?;
        return Ok(());
    }

    let url = format!(
        "https://api.mymemory.translated.net/get?q={}&langpair=en|{}",
        url_encode(text),
        url_encode(lang)
    );

    let data = match http::fetch_json(&url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Translate API failed: {}", e);
            msg.reply("⚠️ Could not translate text right now.").await?;
            return Ok(());
        }
    };

    let translated = data["responseData"]["translatedText"]
        .as_str()
        .unwrap_or("Could not parse translation.");

    msg.reply(&format!("🌍 ({} → {}): {}", "EN", lang.to_uppercase(), translated)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !define <word> — Dictionary definition via Free Dictionary API
// -----------------------------------------------------------------------------

pub async fn define_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let word = args.trim();

    if word.is_empty() {
        msg.reply("Usage: !define <word>\nExample: !define serendipity").await?;
        return Ok(());
    }

    let url = format!(
        "https://api.dictionaryapi.dev/api/v2/entries/en/{}",
        url_encode(word)
    );

    let data = match http::fetch_json(&url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Dictionary API failed: {}", e);
            msg.reply(&format!("⚠️ No definition found for \"{}\".", word)).await?;
            return Ok(());
        }
    };

    // Response is an array of entries — take the first
    let entries = match data.as_array() {
        Some(arr) if !arr.is_empty() => &arr[0],
        _ => {
            msg.reply(&format!("⚠️ No definition found for \"{}\".", word)).await?;
            return Ok(());
        }
    };

    let phonetic = entries["phonetic"].as_str().unwrap_or("");
    let meanings = entries["meanings"].as_array();

    let mut response = format!("📖 **{}**", word);
    if !phonetic.is_empty() {
        response.push_str(&format!(" {}", phonetic));
    }
    response.push('\n');

    let mut shown = 0;
    if let Some(meanings) = meanings {
        for meaning in meanings {
            if shown >= 3 {
                break;
            }
            let pos = meaning["partOfSpeech"].as_str().unwrap_or("?");
            if let Some(defs) = meaning["definitions"].as_array() {
                for def in defs {
                    if shown >= 3 {
                        break;
                    }
                    if let Some(definition) = def["definition"].as_str() {
                        response.push_str(&format!("  *({})* {}\n", pos, definition));
                        shown += 1;
                    }
                }
            }
        }
    }

    if shown == 0 {
        response.push_str("  No definitions available.");
    }

    msg.reply(response.trim()).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !quote — Random inspirational quote via Quotable API
// -----------------------------------------------------------------------------

pub async fn quote_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let url = "https://api.quotable.io/random";

    let data = match http::fetch_json(url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Quote API failed: {}", e);
            msg.reply("⚠️ Could not fetch a quote right now.").await?;
            return Ok(());
        }
    };

    let content = data["content"].as_str().unwrap_or("...");
    let author = data["author"].as_str().unwrap_or("Unknown");

    msg.reply(&format!("💬 \"{}\" — {}", content, author)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !joke — Random dad joke via icanhazdadjoke.com
// -----------------------------------------------------------------------------

pub async fn joke_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    // icanhazdadjoke.com requires Accept: application/json header.
    // The shared HTTP client in lib/http.rs doesn't set this, so we make
    // a one-off request with the correct header.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("concord-bots/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .get("https://icanhazdadjoke.com/")
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            match r.json::<serde_json::Value>().await {
                Ok(data) => {
                    let joke = data["joke"].as_str().unwrap_or("Could not parse joke.");
                    msg.reply(&format!("😄 {}", joke)).await?;
                }
                Err(_) => {
                    msg.reply("⚠️ Could not parse joke response.").await?;
                }
            }
        }
        _ => {
            msg.reply("⚠️ Could not fetch a joke right now.").await?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// !fact — Random fun fact via Useless Facts API
// -----------------------------------------------------------------------------

pub async fn fact_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let url = "https://uselessfacts.jsph.pl/api/v2/facts/random?language=en";

    let data = match http::fetch_json(url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Facts API failed: {}", e);
            msg.reply("⚠️ Could not fetch a fact right now.").await?;
            return Ok(());
        }
    };

    let fact = data["text"].as_str().unwrap_or("Could not parse fact.");

    msg.reply(&format!("🧠 {}", fact)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !meme — Random meme via meme-api.com
// -----------------------------------------------------------------------------

pub async fn meme_command(_ctx: &BotContext, msg: &IncomingMessage) -> Result<()> {
    let url = "https://meme-api.com/gimme";

    let data = match http::fetch_json(url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Meme API failed: {}", e);
            msg.reply("⚠️ Could not fetch a meme right now.").await?;
            return Ok(());
        }
    };

    let title = data["title"].as_str().unwrap_or("A meme");
    let meme_url = data["url"].as_str().unwrap_or("");
    let subreddit = data["subreddit"].as_str().unwrap_or("");

    let mut response = format!("🤣 **{}**", title);
    if !subreddit.is_empty() {
        response.push_str(&format!(" (r/{})", subreddit));
    }
    if !meme_url.is_empty() {
        response.push_str(&format!("\n{}", meme_url));
    }

    msg.reply(&response).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// !shorten <url> — URL shortener via is.gd
// -----------------------------------------------------------------------------

pub async fn shorten_command(_ctx: &BotContext, msg: &IncomingMessage, args: &str) -> Result<()> {
    let url = args.trim();

    if url.is_empty() {
        msg.reply("Usage: !shorten <url>\nExample: !shorten https://example.com/very/long/url").await?;
        return Ok(());
    }

    // Basic URL validation
    if !url.starts_with("http://") && !url.starts_with("https://") {
        msg.reply("⚠️ URL must start with http:// or https://").await?;
        return Ok(());
    }

    let api_url = format!(
        "https://is.gd/create.php?format=json&url={}",
        url_encode(url)
    );

    let data = match http::fetch_json(&api_url).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("is.gd API failed: {}", e);
            msg.reply("⚠️ Could not shorten URL right now.").await?;
            return Ok(());
        }
    };

    // Check for error response
    if let Some(err_msg) = data["errormessage"].as_str() {
        msg.reply(&format!("⚠️ {}", err_msg)).await?;
        return Ok(());
    }

    let short = data["shorturl"].as_str().unwrap_or("Could not parse response.");

    msg.reply(&format!("🔗 {}", short)).await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// URL encoding helper (avoids adding a new dependency)
// -----------------------------------------------------------------------------

fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
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

    #[test]
    fn test_parse_reminder_input() {
        // Short format
        let (desc, msg) = parse_reminder_input("30m Call mom").unwrap();
        assert_eq!(desc, "30 minutes");
        assert_eq!(msg, "Call mom");

        // Long format
        let (desc, msg) = parse_reminder_input("2 hours Check the oven").unwrap();
        assert_eq!(desc, "2 hours");
        assert_eq!(msg, "Check the oven");

        // Day format
        let (desc, msg) = parse_reminder_input("1d Pay bills").unwrap();
        assert_eq!(desc, "1 day");
        assert_eq!(msg, "Pay bills");

        // Singular
        let (desc, _) = parse_reminder_input("1 minute Test").unwrap();
        assert_eq!(desc, "1 minute");

        // Invalid
        assert!(parse_reminder_input("notime here").is_none());
        assert!(parse_reminder_input("").is_none());
    }

    #[test]
    fn test_number_emoji() {
        assert_eq!(number_emoji(1), "1⃣");
        assert_eq!(number_emoji(5), "5⃣");
        assert_eq!(number_emoji(10), "🔟");
        assert_eq!(number_emoji(99), "❓");
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("https://example.com/path?q=1&a=b"), "https%3A%2F%2Fexample.com%2Fpath%3Fq%3D1%26a%3Db");
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
    }
}
