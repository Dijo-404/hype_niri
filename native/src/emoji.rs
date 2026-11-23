use crate::{Result, dialog, err, ipc, keyboard};
use gtk::{gdk, prelude::*};
use serde_json::json;
use std::time::{Duration, Instant};

fn choices() -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    for emoji in emojis::iter() {
        let variants: Vec<_> = emoji
            .skin_tones()
            .map(|tones| tones.collect())
            .unwrap_or_else(|| vec![emoji]);
        for variant in variants {
            let aliases = variant.shortcodes().collect::<Vec<_>>().join(" ");
            rows.push((
                variant.as_str(),
                format!("{}  {}  {}", variant.as_str(), variant.name(), aliases),
            ));
        }
    }
    rows
}
fn focused_window() -> Result<Option<u64>> {
    let reply = ipc::request(json!("FocusedWindow"))?;
    Ok(reply
        .get("FocusedWindow")
        .and_then(|window| window.get("id"))
        .and_then(serde_json::Value::as_u64))
}
pub async fn run(args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err(err("Usage: hype-control emoji"));
    }
    let target = if std::env::var_os("NIRI_SOCKET").is_some() {
        Some(focused_window()?)
    } else {
        None
    };
    let rows = choices();
    let labels: Vec<_> = rows.iter().map(|(_, label)| label.clone()).collect();
    let Some(index) = dialog::choose("Search emoji", &labels)? else {
        return Ok(());
    };
    // Process the picker's destroy request before returning focus to the previous client.
    if let Some(display) = gdk::Display::default() {
        display.sync();
    }
    if let Some(target) = target {
        let target = target.ok_or_else(|| err("No focused application to receive the emoji"))?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while focused_window()? != Some(target) {
            if Instant::now() >= deadline {
                return Err(err("Emoji target did not regain focus"));
            }
            gtk::glib::timeout_future(Duration::from_millis(10)).await;
        }
    }
    keyboard::type_text(rows[index].0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn searchable_metadata_keeps_full_emoji_sequences_and_skin_tones() {
        let rows = choices();
        assert!(
            rows.iter()
                .any(|(emoji, label)| *emoji == "👩🏽‍💻" && label.contains("woman technologist"))
        );
        assert!(
            rows.iter()
                .any(|(emoji, label)| *emoji == "❤️" && label.contains("heart"))
        );
        assert!(rows.iter().any(|(emoji, _)| *emoji == "🇮🇳"));
        let keys: std::collections::HashSet<_> = rows.iter().map(|(emoji, _)| *emoji).collect();
        assert_eq!(rows.len(), keys.len());
    }
}
