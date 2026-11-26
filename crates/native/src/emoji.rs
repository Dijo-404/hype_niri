use crate::{Result, dialog, err, ipc, keyboard, runtime};
use gtk::{gdk, prelude::*};
use serde_json::json;
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    env,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_LIST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HISTORY_BYTES: u64 = 1024 * 1024;
type Choice = (String, String);

fn choices() -> Vec<Choice> {
    let mut rows = Vec::new();
    for emoji in emojis::iter() {
        let variants: Vec<_> = emoji
            .skin_tones()
            .map(|tones| tones.collect())
            .unwrap_or_else(|| vec![emoji]);
        for variant in variants {
            let aliases = variant.shortcodes().collect::<Vec<_>>().join(" ");
            rows.push((
                variant.as_str().into(),
                format!("{}  {}  {}", variant.as_str(), variant.name(), aliases),
            ));
        }
    }
    rows
}
fn option_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
fn history_path() -> PathBuf {
    option_path("BEMOJI_HISTORY_LOCATION")
        .unwrap_or_else(runtime::state_dir)
        .join("bemoji-history.txt")
}
fn parse_choices(text: &str) -> Vec<Choice> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let value = line.split_whitespace().next()?;
            if value.len() > 128 || line.len() > 1024 || value.chars().any(char::is_control) {
                return None;
            }
            Some((value.into(), line.into()))
        })
        .collect()
}
fn read_text(path: &Path, max_bytes: u64, tail: bool) -> Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() {
        return Err(err("Expected a regular emoji list or history file"));
    }
    let skip = tail && info.len() > max_bytes;
    if skip {
        file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + u64::from(!tail))
        .read_to_end(&mut bytes)?;
    if !tail && bytes.len() as u64 > max_bytes {
        return Err(err("Emoji list exceeds size limit"));
    }
    let bytes = if skip {
        let Some(start) = bytes.iter().position(|byte| *byte == b'\n') else {
            return Ok(String::new());
        };
        &bytes[start + 1..]
    } else {
        &bytes
    };
    Ok(String::from_utf8_lossy(bytes).into_owned())
}
fn recent_choices(text: &str, limit: usize) -> Vec<Choice> {
    let mut entries = HashMap::new();
    for (position, (symbol, label)) in parse_choices(text).into_iter().enumerate() {
        let entry = entries.entry(symbol).or_insert((0usize, 0, String::new()));
        entry.0 += 1;
        entry.1 = position;
        entry.2 = label;
    }
    let mut rows: Vec<_> = entries.into_iter().collect();
    rows.sort_unstable_by_key(|(_, (count, last, _))| (Reverse(*count), Reverse(*last)));
    rows.into_iter()
        .take(limit)
        .map(|(symbol, (_, _, label))| (symbol, label))
        .collect()
}
fn custom_choices() -> Result<Vec<Choice>> {
    if let Some(path) = option_path("BEMOJI_CUSTOM_LIST") {
        if let Some(url) = path
            .to_str()
            .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
        {
            let response = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()?
                .get(url)
                .send()?
                .error_for_status()?;
            let mut bytes = Vec::new();
            response.take(MAX_LIST_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_LIST_BYTES {
                return Err(err("Emoji list exceeds size limit"));
            }
            return Ok(parse_choices(&String::from_utf8_lossy(&bytes)));
        }
        return Ok(parse_choices(&read_text(&path, MAX_LIST_BYTES, false)?));
    }
    let directory = option_path("BEMOJI_DB_LOCATION")
        .unwrap_or_else(|| gtk::glib::user_data_dir().join("bemoji"));
    let mut paths = match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    paths.sort();
    let mut rows = Vec::new();
    for path in paths
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
    {
        match read_text(&path, MAX_LIST_BYTES, false) {
            Ok(text) => rows.extend(parse_choices(&text)),
            Err(error) => eprintln!("Emoji list could not be read: {error}"),
        }
    }
    Ok(rows)
}
fn picker_choices(history: &Path) -> Vec<Choice> {
    let limit = env::var("BEMOJI_LIMIT_RECENT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(usize::MAX);
    let history = if limit == 0 {
        Vec::new()
    } else {
        read_text(history, MAX_HISTORY_BYTES, true)
            .map(|text| recent_choices(&text, limit))
            .unwrap_or_default()
    };
    let mut rows = match custom_choices() {
        Ok(mut rows) => {
            for (symbol, label) in &mut rows {
                if let Some(emoji) = emojis::get(symbol) {
                    for alias in emoji.shortcodes() {
                        if !label.split_whitespace().any(|word| word == alias) {
                            label.push(' ');
                            label.push_str(alias);
                        }
                    }
                }
            }
            if option_path("BEMOJI_CUSTOM_LIST").is_none() {
                rows.extend(choices());
            }
            rows
        }
        Err(error) => {
            eprintln!("Emoji lists could not be read: {error}");
            choices()
        }
    };
    // Keep descriptions and aliases from the catalog when an emoji is also in history.
    let catalog: HashMap<_, _> = rows
        .iter()
        .rev()
        .map(|(value, label)| (value, label))
        .collect();
    let mut recent: Vec<_> = history
        .into_iter()
        .map(|(value, label)| {
            let label = catalog.get(&value).map_or(label, |found| (*found).clone());
            (value, label)
        })
        .collect();
    recent.append(&mut rows);
    let mut seen = HashSet::new();
    recent.retain(|(value, _)| seen.insert(value.clone()));
    recent
}
fn remember(path: &Path, label: &str) -> Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| err("Missing emoji history directory"))?,
    )?;
    let mut history = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = history.metadata()?;
    if !metadata.is_file() || metadata.uid() != runtime::uid() || metadata.nlink() != 1 {
        return Err(err("Unsafe emoji history file"));
    }
    if metadata.mode() & 0o077 != 0 {
        history.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let mut separator = "";
    if metadata.len() > 0 {
        history.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        history.read_exact(&mut last)?;
        if last[0] != b'\n' {
            separator = "\n";
        }
    }
    history.write_all(format!("{separator}{label}\n").as_bytes())?;
    Ok(())
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
    let history = history_path();
    let state = history.clone();
    let rows = gtk::gio::spawn_blocking(move || picker_choices(&state))
        .await
        .map_err(|_| err("Could not load the emoji picker"))?;
    let labels: Vec<_> = rows.iter().map(|(_, label)| label.clone()).collect();
    let Some(index) = dialog::choose("Search emoji", &labels).await? else {
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
    keyboard::type_text(&rows[index].0)?;
    if env::var("BEMOJI_PRIVATE_MODE").as_deref() != Ok("true")
        && let Err(error) = remember(&history, &rows[index].1)
    {
        eprintln!("Emoji history was not saved: {error}");
    }
    Ok(())
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
        let keys: HashSet<_> = rows.iter().map(|(emoji, _)| emoji).collect();
        assert_eq!(rows.len(), keys.len());
    }
    #[test]
    fn legacy_history_ranks_by_frequency_then_latest_choice() {
        let history = "\n🦊 Fox\n🥑 Avocado\n🦊 Fox updated\n#️⃣ Keycap\n";
        let rows = recent_choices(history, 2);
        assert_eq!(
            rows,
            [
                ("🦊".into(), "🦊 Fox updated".into()),
                ("#️⃣".into(), "#️⃣ Keycap".into())
            ]
        );
        let custom = parse_choices("󰣇 Arch Linux\nλ lambda\n👩🏽‍💻 Woman technologist \n");
        assert_eq!(custom[0], ("󰣇".into(), "󰣇 Arch Linux".into()));
        assert_eq!(custom[2], ("👩🏽‍💻".into(), "👩🏽‍💻 Woman technologist".into()));
    }
    #[test]
    fn bounded_history_reads_only_complete_recent_lines() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("history");
        fs::write(&file, "old old\nα Alpha\n").unwrap();
        assert_eq!(read_text(&file, 12, true).unwrap(), "α Alpha\n");
        assert!(read_text(&file, 12, false).is_err());
    }
    #[test]
    fn appending_to_bemoji_history_preserves_previous_entries_and_privacy() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("bemoji-history.txt");
        fs::write(&history, "🦊 Fox").unwrap();
        fs::set_permissions(&history, fs::Permissions::from_mode(0o644)).unwrap();
        remember(&history, "🥑 Avocado").unwrap();
        assert_eq!(
            fs::read_to_string(&history).unwrap(),
            "🦊 Fox\n🥑 Avocado\n"
        );
        assert_eq!(fs::metadata(history).unwrap().mode() & 0o777, 0o600);
    }
}
