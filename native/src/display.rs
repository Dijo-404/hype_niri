//! Native Niri output configuration. Validation uses the compositor's own pinned decoder.
use crate::{Result, err, ipc, runtime};
use kdl::KdlDocument;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Deserialize)]
struct Mode {
    width: u32,
    height: u32,
    refresh_rate: u32,
    #[serde(default)]
    is_preferred: bool,
}
#[derive(Clone, Debug, Deserialize)]
struct Output {
    name: String,
    #[serde(default = "unknown")]
    make: String,
    #[serde(default = "unknown")]
    model: String,
    serial: Option<String>,
    current_mode: Option<usize>,
    #[serde(default)]
    modes: Vec<Mode>,
}
fn unknown() -> String {
    "Unknown".into()
}
impl Output {
    fn id(&self) -> String {
        if self.make == "Unknown" && self.model == "Unknown" && self.serial.is_none() {
            self.name.clone()
        } else {
            format!(
                "{} {} {}",
                self.make,
                self.model,
                self.serial.as_deref().unwrap_or("Unknown")
            )
        }
    }
}
fn scale(width: u32, height: u32) -> &'static str {
    match width.min(height) {
        1800.. => "2.0",
        1600.. => "1.5",
        _ => "1.0",
    }
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn outputs() -> Result<Vec<Output>> {
    let response = ipc::request(json!("Outputs"))?;
    let value = response.get("Outputs").unwrap_or(&response);
    Ok(
        serde_json::from_value::<BTreeMap<String, Output>>(value.clone())?
            .into_values()
            .collect(),
    )
}
fn outputs_path() -> PathBuf {
    runtime::config_dir().join("niri/outputs.kdl")
}
fn read(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}
fn doc(text: &str) -> Result<KdlDocument> {
    KdlDocument::parse_v1(text).map_err(|e| err(e.to_string()))
}
fn matches(node: &kdl::KdlNode, id: &str, connector: &str) -> bool {
    node.name().value() == "output"
        && node
            .get(0)
            .and_then(|v| v.as_string())
            .is_some_and(|s| s == id || s == connector)
}
fn has_block(text: &str, id: &str, connector: &str) -> Result<bool> {
    Ok(doc(text)?.nodes().iter().any(|n| matches(n, id, connector)))
}
fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |n| n + 1)
}
fn line_end(text: &str, pos: usize) -> usize {
    text[pos..].find('\n').map_or(text.len(), |n| pos + n + 1)
}
fn check_format(text: &str, node: &kdl::KdlNode) -> Result<()> {
    let offset = node.span().offset();
    let header = &text[offset..line_end(text, offset)];
    if node.children().is_none() || !header.trim_end().ends_with('{') {
        return Err(err(
            "Unsupported output block format: keep the output header on its own line and put each setting on a separate line.",
        ));
    }
    Ok(())
}
fn upsert(text: &str, id: &str, connector: &str, settings: &[String]) -> Result<String> {
    let parsed = doc(text)?;
    let mut removed = Vec::new();
    let mut kept = String::new();
    let mut seen = HashSet::new();
    let replaced: HashSet<&str> = settings
        .iter()
        .filter_map(|s| s.split_whitespace().next())
        .collect();
    for node in parsed.nodes().iter().filter(|n| matches(n, id, connector)) {
        check_format(text, node)?;
        let start = node.span().offset();
        let end = start + node.span().len();
        removed.push((line_start(text, start), line_end(text, end)));
        let interior_start = line_end(text, start);
        let interior_end = text[..end]
            .rfind('}')
            .ok_or_else(|| err("Output block is not closed"))?;
        let mut cursor = interior_start;
        for child in node.children().unwrap().nodes() {
            let a = line_start(text, child.span().offset());
            let b = line_end(text, child.span().offset() + child.span().len());
            if a < cursor || b > interior_end {
                return Err(err("Put each output setting on its own line"));
            }
            kept.push_str(&text[cursor..a]);
            let key = child.name().value();
            if child.children().is_some()
                || (!replaced.contains(key) && seen.insert(key.to_owned()))
            {
                kept.push_str(&text[a..b]);
            }
            cursor = b;
        }
        kept.push_str(&text[cursor..interior_end]);
    }
    let mut out = String::new();
    let mut cursor = 0;
    for (a, b) in removed {
        out.push_str(&text[cursor..a]);
        cursor = b;
    }
    out.push_str(&text[cursor..]);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("output {} {{\n", quote(id)));
    for s in settings {
        out.push_str("    ");
        out.push_str(s);
        out.push('\n');
    }
    out.push_str(&kept);
    out.push_str("}\n");
    Ok(out)
}
fn validate(path: &Path, text: &str) -> Result<()> {
    niri_config::Config::parse(path, text)
        .config
        .map(|_| ())
        .map_err(|e| err(format!("Invalid Niri configuration: {e:?}")))
}
fn reload() -> Result<()> {
    ipc::action(json!({"LoadConfigFile":{"path":null}}))
}
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for p in path.components() {
        match p {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            p => out.push(p.as_os_str()),
        }
    }
    out
}
// Stage the complete include graph so the actual opacity file is never published before validation.
fn validate_replacement(root: &Path, target: &Path, replacement: &str) -> Result<()> {
    let stage = tempfile::tempdir()?;
    fn stage_file(
        path: &Path,
        target: &Path,
        replacement: &str,
        dir: &Path,
        stack: &mut HashSet<PathBuf>,
        counter: &mut usize,
    ) -> Result<PathBuf> {
        let path = normalized(path);
        if stack.len() >= 32 || !stack.insert(path.clone()) {
            return Err(err("Recursive Niri include"));
        }
        let mut text = if path == normalized(target) {
            replacement.to_owned()
        } else {
            fs::read_to_string(&path)?
        };
        let parsed = doc(&text)?;
        let mut edits = Vec::new();
        for node in parsed
            .nodes()
            .iter()
            .filter(|n| n.name().value() == "include")
        {
            let entry = node
                .entry(0)
                .ok_or_else(|| err("Niri include needs a path"))?;
            let s = entry
                .value()
                .as_string()
                .ok_or_else(|| err("Niri include path must be a string"))?;
            let child = if let Some(s) = s.strip_prefix("~/") {
                runtime::home()?.join(s)
            } else {
                path.parent().unwrap_or(Path::new(".")).join(s)
            };
            let optional = node
                .get("optional")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let staged = if !child.exists() && normalized(&child) != normalized(target) && optional
            {
                dir.join(format!("missing-{}", *counter))
            } else {
                stage_file(&child, target, replacement, dir, stack, counter)?
            };
            let span = entry.span();
            edits.push((
                span.offset(),
                span.len(),
                quote(
                    staged
                        .to_str()
                        .ok_or_else(|| err("Non-UTF8 config include path"))?,
                ),
            ));
        }
        for (offset, len, value) in edits.into_iter().rev() {
            text.replace_range(offset..offset + len, &value);
        }
        let dest = dir.join(format!("config-{}.kdl", *counter));
        *counter += 1;
        fs::write(&dest, text)?;
        stack.remove(&path);
        Ok(dest)
    }
    let staged = stage_file(
        root,
        target,
        replacement,
        stage.path(),
        &mut HashSet::new(),
        &mut 0,
    )?;
    niri_config::Config::load(&staged)
        .config
        .map(|_| ())
        .map_err(|e| err(format!("Invalid Niri configuration: {e:?}")))
}
async fn auto_scale(args: &[String]) -> Result<()> {
    let Some(_guard) = runtime::lock("hype-niri-outputs.lock", Duration::from_secs(5))? else {
        return Ok(());
    };
    let outputs = match outputs() {
        Ok(o) => o,
        Err(_) => return Ok(()),
    };
    let path = outputs_path();
    let old = read(&path)?;
    let mut next = old.clone();
    let mut active: Vec<_> = outputs
        .iter()
        .filter_map(|o| o.current_mode.and_then(|i| o.modes.get(i)).map(|m| (o, m)))
        .collect();
    active.sort_by_key(|(o, _)| o.id());
    for (o, m) in active {
        let id = o.id();
        for n in doc(&next)?
            .nodes()
            .iter()
            .filter(|n| matches(n, &id, &o.name))
        {
            check_format(&next, n)?;
        }
        if !has_block(&next, &id, &o.name)? {
            next = upsert(
                &next,
                &id,
                &o.name,
                &[format!("scale {}", scale(m.width, m.height))],
            )?;
        } else if id != o.name && has_block(&next, &o.name, &o.name)? {
            next = upsert(&next, &id, &o.name, &[])?;
        }
    }
    if next != old {
        validate(&path, &next)?;
        fs::create_dir_all(path.parent().unwrap())?;
        runtime::atomic_write(&path, next.as_bytes())?;
        if !args.iter().any(|a| a == "--no-reload") {
            let _ = reload();
        }
    }
    Ok(())
}
fn rate(mhz: u32) -> String {
    format!("{}.{:03}", mhz / 1000, mhz % 1000)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}
fn display_rate(mhz: u32) -> String {
    let hz = mhz as f64 / 1000.;
    if (hz - hz.round()).abs() < 0.01 {
        format!("{:.0}", hz)
    } else {
        format!("{hz:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}
fn modes(o: &Output) -> Vec<(Mode, bool)> {
    let mut modes: BTreeMap<(u32, u32, u32), (Mode, bool)> = BTreeMap::new();
    for (i, m) in o.modes.iter().enumerate() {
        let entry = modes
            .entry((m.width, m.height, m.refresh_rate))
            .or_insert((m.clone(), false));
        entry.0.is_preferred |= m.is_preferred;
        entry.1 |= o.current_mode == Some(i);
    }
    modes.into_values().rev().collect()
}
async fn refresh() -> Result<()> {
    let outputs = outputs()?;
    if outputs.is_empty() {
        return Err(err("No monitors found"));
    }
    let labels: Vec<_> = outputs
        .iter()
        .map(|o| {
            let current = o
                .current_mode
                .and_then(|i| o.modes.get(i))
                .map(|m| {
                    format!(
                        "{}x{} @ {} Hz",
                        m.width,
                        m.height,
                        display_rate(m.refresh_rate)
                    )
                })
                .unwrap_or("current mode unknown".into());
            format!(
                "{:<7} {:<24} {current}",
                o.name,
                format!("{} {}", o.make, o.model).trim()
            )
        })
        .collect();
    let Some(index) = crate::dialog::choose("Monitor", &labels).await? else {
        return Ok(());
    };
    let output = outputs
        .get(index)
        .ok_or_else(|| err("Invalid monitor selection"))?;
    let modes = modes(output);
    if modes.is_empty() {
        return Err(err("No refresh rates found"));
    }
    let labels: Vec<_> = modes
        .iter()
        .map(|(m, current)| {
            let state = match (*current, m.is_preferred) {
                (true, true) => "  [current, preferred]",
                (true, false) => "  [current]",
                (false, true) => "  [preferred]",
                _ => "",
            };
            format!(
                "{:<11} {:>8} Hz{state}",
                format!("{}x{}", m.width, m.height),
                display_rate(m.refresh_rate)
            )
        })
        .collect();
    let Some(index) = crate::dialog::choose(&format!("{} refresh", output.name), &labels).await?
    else {
        return Ok(());
    };
    let (mode, _) = modes
        .get(index)
        .ok_or_else(|| err("Invalid mode selection"))?;
    let mode_name = format!("{}x{}@{}", mode.width, mode.height, rate(mode.refresh_rate));
    let scale = scale(mode.width, mode.height);
    let Some(_guard) = runtime::lock("hype-niri-outputs.lock", Duration::from_secs(5))? else {
        return Ok(());
    };
    let path = outputs_path();
    let original = read(&path)?;
    let next = upsert(
        &original,
        &output.id(),
        &output.name,
        &[
            format!("mode {}", quote(&mode_name)),
            format!("scale {scale}"),
        ],
    )?;
    validate(&path, &next)?;
    ipc::request(
        json!({"Output":{"output":output.name,"action":{"Mode":{"mode":{"Specific":{"width":mode.width,"height":mode.height,"refresh":mode.refresh_rate as f64/1000.}}}}}}),
    )?;
    ipc::request(
        json!({"Output":{"output":output.name,"action":{"Scale":{"scale":{"Specific":scale.parse::<f64>()?}}}}}),
    )?;
    fs::create_dir_all(path.parent().unwrap())?;
    runtime::atomic_write(&path, next.as_bytes())?;
    reload()?;
    let _ = crate::notify::send(
        2006,
        "Monitor refresh saved",
        &format!("{} is now {mode_name} at {scale}x scale", output.name),
        None,
        "",
    )
    .await;
    Ok(())
}
fn opacity_config(enabled: bool) -> String {
    let (active, inactive) = if enabled {
        ("0.95", "0.90")
    } else {
        ("1.0", "1.0")
    };
    format!(
        "window-rule {{\n    match is-active=true\n    draw-border-with-background false\n    opacity {active}\n}}\n\nwindow-rule {{\n    match is-active=false\n    draw-border-with-background false\n    opacity {inactive}\n}}\n"
    )
}
fn opaque(text: &str) -> bool {
    text.lines().any(|line| {
        let parts: Vec<_> = line.split_whitespace().collect();
        parts.len() == 2
            && parts[0] == "opacity"
            && (parts[1] == "1"
                || parts[1]
                    .strip_prefix("1.")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b == b'0')))
    })
}
async fn toggle_opacity() -> Result<()> {
    let Some(_guard) = runtime::lock("hype-opacity-toggle.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let config = std::env::var_os("NIRI_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime::config_dir().join("niri/config.kdl"));
    if !config.is_file() {
        return Err(err(format!("Niri config not found: {}", config.display())));
    }
    let path = config.parent().unwrap().join("opacity.kdl");
    let existed = path.exists();
    let original = read(&path)?;
    let enabled = opaque(&original);
    let next = opacity_config(enabled);
    validate_replacement(&config, &path, &next)?;
    runtime::atomic_write(&path, next.as_bytes())?;
    if let Err(error) = reload() {
        if existed {
            runtime::atomic_write(&path, original.as_bytes())?;
        } else {
            fs::remove_file(&path)?;
        }
        let _ = reload();
        return Err(error);
    }
    let (title, body) = if enabled {
        ("Window opacity enabled", "Active: 95% · Inactive: 90%")
    } else {
        ("Window opacity disabled", "Windows are now fully opaque.")
    };
    let _ = crate::notify::send(2007, title, body, None, "").await;
    Ok(())
}
pub async fn run(command: &str, args: &[String]) -> Result<()> {
    match command {
        "display-scale" | "niri-outputs" => auto_scale(args).await,
        "monitor-refresh" => refresh().await,
        "opacity-toggle" => toggle_opacity().await,
        _ => Err(err(format!("Unknown display command: {command}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scale_thresholds() {
        for (w, h, expected) in [
            (2880, 1800, "2.0"),
            (1600, 2560, "1.5"),
            (3840, 1599, "1.0"),
            (0, 2000, "1.0"),
        ] {
            assert_eq!(scale(w, h), expected);
        }
    }
    #[test]
    fn preserves_settings_and_adopts_stable_id() {
        let old = "output \"DP-2\" {\n    mode \"2560x1440@59.951\"\n    position x=0 y=0\n}\noutput \"laptop\" {\n    scale 2.0\n}\n";
        let new = upsert(
            old,
            "Dell Inc. DELL U2723QE ABC123",
            "DP-2",
            &["mode \"2560x1440@165\"".into(), "scale 1.0".into()],
        )
        .unwrap();
        assert_eq!(
            new,
            "output \"laptop\" {\n    scale 2.0\n}\noutput \"Dell Inc. DELL U2723QE ABC123\" {\n    mode \"2560x1440@165\"\n    scale 1.0\n    position x=0 y=0\n}\n"
        );
        validate(Path::new("outputs.kdl"), &new).unwrap();
    }
    #[test]
    fn compact_output_rejected_and_braces_in_strings_preserved() {
        assert!(upsert("output \"DP-2\" { scale 1.5; }\n", "stable", "DP-2", &[]).is_err());
        let id = "Make \"quoted\" \\ serial";
        let text = format!("output {} {{\n    scale 1.5\n}}\n", quote(id));
        let edited = upsert(&text, id, "DP-2", &["mode \"1920x1080@60\"".into()]).unwrap();
        assert!(edited.contains("scale 1.5"));
        validate(Path::new("outputs.kdl"), &edited).unwrap();
    }
    #[test]
    fn semantic_validation_rejects_unknown_and_invalid_scale() {
        assert!(
            validate(
                Path::new("outputs.kdl"),
                "output \"DP-1\" {\n scale -1\n}\n"
            )
            .is_err()
        );
        assert!(validate(Path::new("outputs.kdl"), "made-up-option 1\n").is_err());
    }
    #[test]
    fn opacity_overlay_checks_includes_before_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("config.kdl");
        let opacity = temp.path().join("opacity.kdl");
        fs::write(&root, "include \"nested.kdl\"\n").unwrap();
        fs::write(temp.path().join("nested.kdl"), "include \"opacity.kdl\"\n").unwrap();
        fs::write(&opacity, "broken original\n").unwrap();
        let next = opacity_config(true);
        validate_replacement(&root, &opacity, &next).unwrap();
        assert_eq!(fs::read_to_string(&opacity).unwrap(), "broken original\n");
        assert!(
            validate_replacement(&root, &opacity, "window-rule {\n unknown-rule 3.0\n}\n").is_err()
        );
    }
    #[test]
    fn deduplicates_modes_and_formats_fractional_refresh() {
        let o:Output=serde_json::from_value(json!({"name":"DP-1","current_mode":1,"modes":[{"width":1920,"height":1080,"refresh_rate":59951},{"width":1920,"height":1080,"refresh_rate":59951,"is_preferred":true}]})).unwrap();
        let m = modes(&o);
        assert_eq!(m.len(), 1);
        assert!(m[0].0.is_preferred && m[0].1);
        assert_eq!(rate(59951), "59.951");
        assert_eq!(display_rate(59951), "59.95");
        assert_eq!(rate(60000), "60");
    }
    #[test]
    fn opacity_toggle_recognizes_exact_legacy_opaque_values() {
        for value in ["1", "1.0", "1.000"] {
            assert!(opaque(&format!("  opacity {value}  ")));
        }
        for value in ["0.95", "01", "1e0", "1.", "1.0 // comment"] {
            assert!(!opaque(&format!("opacity {value}")));
        }
    }
}
