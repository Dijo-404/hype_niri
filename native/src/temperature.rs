use crate::{Result, runtime};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Sensor {
    path: PathBuf,
    resolved: PathBuf,
    name: String,
    label: String,
}

fn first_line(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .next()
        .map(str::to_owned)
}

fn celsius(raw: &str) -> Option<i64> {
    let raw: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = raw.strip_prefix('-').unwrap_or(&raw);
    if raw.len() > 10 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let raw: i64 = raw.parse().ok()?;
    let value = if raw.abs() > 1000 {
        (raw + if raw < 0 { -500 } else { 500 }) / 1000
    } else {
        raw
    };
    (-40..=150).contains(&value).then_some(value)
}

fn score(name: &str, label: &str, path: &Path) -> i32 {
    let name = name.to_lowercase();
    let label = label.to_lowercase();
    let haystack = format!("{name} {label} {}", path.display()).to_lowercase();
    let contains = |text: &str, terms: &[&str]| terms.iter().any(|term| text.contains(term));
    let mut value = if contains(
        &haystack,
        &[
            "nvme", "iwlwifi", "wifi", "wireless", "bat", "battery", "ucsi", "usb", "charger",
            "adapter", "amdgpu", "radeon", "nouveau", "nvidia", "gpu", "drm",
        ],
    ) {
        -300
    } else {
        0
    };
    value += if contains(
        &name,
        &[
            "coretemp",
            "k10temp",
            "zenpower",
            "cpu_thermal",
            "x86_pkg_temp",
            "fam15h_power",
        ],
    ) {
        120
    } else if contains(&name, &["acpitz", "thermal", "soc"]) {
        20
    } else {
        0
    };
    value
        + if contains(&label, &["package", "tdie"]) {
            90
        } else if contains(&label, &["tctl", "cpu"]) {
            80
        } else if label.contains("core") {
            40
        } else if label.is_empty() {
            5
        } else {
            0
        }
}

fn allowed(path: &Path, hwmon: &Path, thermal: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let directory = parent.file_name().and_then(|s| s.to_str()).unwrap_or("");
    (parent.parent() == Some(hwmon)
        && directory.starts_with("hwmon")
        && name.starts_with("temp")
        && name.ends_with("_input"))
        || (parent.parent() == Some(thermal)
            && directory.starts_with("thermal_zone")
            && name == "temp")
}

fn identity(path: &Path, hwmon: &Path, thermal: &Path) -> Option<Sensor> {
    if !allowed(path, hwmon, thermal) {
        return None;
    }
    let directory = path.parent()?;
    let (name, label) = if directory.parent() == Some(hwmon) {
        let name = first_line(&directory.join("name")).unwrap_or_else(|| "hwmon".into());
        let label_name = path.file_name()?.to_str()?.strip_suffix("_input")?;
        (
            name,
            first_line(&directory.join(format!("{label_name}_label"))).unwrap_or_default(),
        )
    } else {
        let name = first_line(&directory.join("type")).unwrap_or_else(|| "thermal".into());
        (name.clone(), name)
    };
    Some(Sensor {
        path: path.into(),
        resolved: path.canonicalize().ok()?,
        name,
        label,
    })
}

fn entries(path: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_sensor(sensor: &Sensor) -> Option<i64> {
    celsius(&first_line(&sensor.path)?)
}

fn select(hwmon: &Path, thermal: &Path, cache: &Path) -> Option<(Sensor, i64)> {
    if let Some(sensor) = fs::read(cache)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Sensor>(&bytes).ok())
        && identity(&sensor.path, hwmon, thermal).as_ref() == Some(&sensor)
        && let Some(value) = read_sensor(&sensor)
    {
        return Some((sensor, value));
    }
    let mut best: Option<(i32, i64, Sensor)> = None;
    let paths = entries(hwmon)
        .into_iter()
        .flat_map(|directory| entries(&directory))
        .chain(
            entries(thermal)
                .into_iter()
                .map(|directory| directory.join("temp")),
        );
    for path in paths {
        let Some(sensor) = identity(&path, hwmon, thermal) else {
            continue;
        };
        let Some(value) = read_sensor(&sensor) else {
            continue;
        };
        let ranking = score(&sensor.name, &sensor.label, &sensor.path);
        if ranking > 0
            && best
                .as_ref()
                .is_none_or(|(s, t, _)| (ranking, value) > (*s, *t))
        {
            best = Some((ranking, value, sensor));
        }
    }
    let (_, value, sensor) = best?;
    if let Ok(bytes) = serde_json::to_vec(&sensor) {
        // Temperature remains available even when its optional cache cannot be written.
        let _ = runtime::atomic_write(cache, &bytes);
    }
    Some((sensor, value))
}

fn output(reading: Option<(Sensor, i64)>, warning: i64, critical: i64) -> Value {
    let Some((sensor, value)) = reading else {
        return json!({"text":"--°", "tooltip":"Temperature unavailable", "class":"missing"});
    };
    let source = if sensor.label.is_empty() {
        sensor.name
    } else {
        format!("{} / {}", sensor.name, sensor.label)
    };
    let source: String = source
        .chars()
        .map(|c| {
            if ['\n', '\r', '\t'].contains(&c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    json!({"text":format!("{value}°"), "tooltip":format!("CPU temperature: {value}°C ({source})"),
        "class":if value >= critical { "critical" } else if value >= warning { "warning" } else { "normal" }})
}

fn threshold(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .filter(|s| (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

pub async fn run(_command: &str, _args: &[String]) -> Result<()> {
    let hwmon = std::env::var_os("HWMON_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/sys/class/hwmon".into());
    let thermal = std::env::var_os("THERMAL_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/sys/class/thermal".into());
    let cache = runtime::dir()?.join("hype-niri-temp-sensor");
    println!(
        "{}",
        output(
            select(&hwmon, &thermal, &cache),
            threshold("HYPE_NIRI_TEMP_WARNING", 70),
            threshold("HYPE_NIRI_TEMP_CRITICAL", 80)
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sensor(root: &Path, directory: &str, name: &str, label: &str, raw: &str) -> PathBuf {
        let directory = root.join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("name"), name).unwrap();
        fs::write(directory.join("temp1_label"), label).unwrap();
        let path = directory.join("temp1_input");
        fs::write(&path, raw).unwrap();
        path
    }
    #[test]
    fn numeric_rounding_and_invalid_readings() {
        for (raw, expected) in [
            ("075000", Some(75)),
            ("44500", Some(45)),
            ("-39500", Some(-40)),
            ("150499", Some(150)),
            ("150500", None),
            ("-41", None),
            ("nan", None),
            ("+45", None),
            ("99999999999", None),
            ("", None),
        ] {
            assert_eq!(celsius(raw), expected, "{raw}");
        }
    }
    #[test]
    fn ranking_prefers_cpu_package_then_hottest_peer() {
        let root = tempfile::tempdir().unwrap();
        let hwmon = root.path().join("hwmon");
        sensor(&hwmon, "hwmon0", "amdgpu", "edge", "100000");
        sensor(&hwmon, "hwmon1", "coretemp", "Core 0", "95000");
        sensor(&hwmon, "hwmon2", "coretemp", "Package id 0", "70000");
        let expected = sensor(&hwmon, "hwmon3", "coretemp", "Package id 1", "85000");
        let (picked, value) = select(
            &hwmon,
            &root.path().join("thermal"),
            &root.path().join("cache"),
        )
        .unwrap();
        assert_eq!(picked.path, expected);
        assert_eq!(value, 85);
    }
    #[test]
    fn cache_revalidates_identity_and_rejects_outside_roots() {
        let root = tempfile::tempdir().unwrap();
        let hwmon = root.path().join("hwmon");
        let thermal = root.path().join("thermal");
        let cache = root.path().join("cache");
        let first = sensor(&hwmon, "hwmon0", "coretemp", "Package id 0", "45000");
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 45);
        fs::write(first.parent().unwrap().join("name"), "amdgpu").unwrap();
        let next = sensor(&hwmon, "hwmon1", "coretemp", "Package id 0", "85000");
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().0.path, next);
        let mut forged = identity(&next, &hwmon, &thermal).unwrap();
        forged.path = root.path().join("secret");
        fs::write(&forged.path, "50000").unwrap();
        forged.resolved = forged.path.clone();
        fs::write(&cache, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 85);
    }
    #[test]
    fn cache_detects_hwmon_device_replacement() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let hwmon = root.path().join("hwmon");
        let thermal = root.path().join("thermal");
        let cache = root.path().join("cache");
        fs::create_dir_all(&hwmon).unwrap();
        let first = sensor(root.path(), "first", "coretemp", "Package", "45000");
        let replacement = sensor(root.path(), "replacement", "coretemp", "Package", "50000");
        symlink(first.parent().unwrap(), hwmon.join("hwmon0")).unwrap();
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 45);
        fs::remove_file(hwmon.join("hwmon0")).unwrap();
        symlink(replacement.parent().unwrap(), hwmon.join("hwmon0")).unwrap();
        sensor(&hwmon, "hwmon1", "coretemp", "Package", "85000");
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 85);
    }
    #[test]
    fn concurrent_scans_publish_complete_cache_and_invalid_readings_rescan() {
        let root = tempfile::tempdir().unwrap();
        let hwmon = root.path().join("hwmon");
        let thermal = root.path().join("thermal");
        let cache = root.path().join("cache");
        let first = sensor(&hwmon, "hwmon0", "coretemp", "Package\t\"CPU\"", "45000");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 45));
            }
        });
        let saved: Sensor = serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
        assert_eq!(saved.path, first);
        fs::write(&first, "invalid").unwrap();
        sensor(&hwmon, "hwmon1", "coretemp", "Core", "85000");
        assert_eq!(select(&hwmon, &thermal, &cache).unwrap().1, 85);
    }
    #[test]
    fn gpu_only_is_missing_and_tooltip_is_json_safe() {
        assert_eq!(output(None, 70, 80)["class"], "missing");
        let sensor = Sensor {
            path: "x".into(),
            resolved: "x".into(),
            name: "coretemp".into(),
            label: "Package\t\"CPU\"".into(),
        };
        let result = output(Some((sensor, 75)), 70, 80);
        assert_eq!(result["class"], "warning");
        assert!(
            result["tooltip"]
                .as_str()
                .unwrap()
                .contains("Package \"CPU\"")
        );
        assert!(score("amdgpu", "edge", Path::new("/sys/hwmon0/temp1_input")) < 0);
    }
}
