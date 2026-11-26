use crate::{Result, bus, dialog, err, notify, runtime};
use gtk::{gio, glib, prelude::*};
use libpulse_binding::volume::Volume;
use pulse::{Device, Pulse};
use serde_json::json;
use std::{
    cell::Cell,
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[path = "backlight.rs"]
pub mod backlight;
#[path = "pulse.rs"]
pub mod pulse;

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const BATTERY: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE: &str = "org.freedesktop.UPower.Device";
const PROFILES: &str = "org.freedesktop.UPower.PowerProfiles";
const PROFILES_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const PROFILE_NAMES: [&str; 3] = ["power-saver", "balanced", "performance"];

fn brightness_target(current: u32, maximum: u32, up: bool) -> u32 {
    let step = ((u64::from(maximum) + 50) / 100).max(1) as u32;
    let floor = u64::from(maximum).div_ceil(100) as u32;
    if up {
        current.saturating_add(step).min(maximum)
    } else if current <= floor {
        current
    } else {
        current.saturating_sub(step).max(floor)
    }
}

fn brightness_ramp(current: u32, target: u32) -> Vec<u32> {
    let diff = i64::from(target) - i64::from(current);
    let steps = diff.unsigned_abs().min(8);
    (1..=steps)
        .map(|i| (i64::from(current) + diff * i as i64 / steps as i64) as u32)
        .collect()
}

async fn brightness(action: &str) -> Result<()> {
    if !matches!(action, "up" | "down") {
        return Err(err("Usage: hype-control brightness {up|down}"));
    }
    let Some(lock) = runtime::lock("brightness-control.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let root = std::env::var_os("BACKLIGHT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/sys/class/backlight".into());
    let Some(light) = backlight::find(&root) else {
        return Ok(());
    };
    let target = brightness_target(light.current, light.maximum, action == "up");
    let ramp = brightness_ramp(light.current, target);
    for (index, value) in ramp.iter().enumerate() {
        backlight::write(&light, *value).await?;
        if index + 1 < ramp.len() {
            glib::timeout_future(Duration::from_millis(5)).await;
        }
    }
    let current = fs::read_to_string(light.path.join("brightness"))?
        .trim()
        .parse::<u32>()?;
    let percent = ((u64::from(current) * 100 + u64::from(light.maximum) / 2)
        / u64::from(light.maximum)) as i32;
    drop(lock);
    let icon = if percent < 30 {
        "󰃞"
    } else if percent < 70 {
        "󰃟"
    } else {
        "󰃠"
    };
    let _ = notify::send(
        2000,
        &format!("{icon}  Brightness: {percent}%"),
        "",
        Some(percent),
        "brightness",
    )
    .await;
    Ok(())
}

fn change_audio(mic: bool, action: &str) -> Result<Device> {
    let mut pulse = Pulse::connect(
        "Hype desktop controls",
        Instant::now() + Duration::from_secs(3),
    )?;
    let mut state = pulse.read(mic)?;
    if action == "mute" {
        pulse.set_mute(mic, state.index, !state.muted)?;
    } else if !mic && matches!(action, "up" | "down") {
        let target = volume_target(state.channels.max().0, action == "up");
        state.channels.scale(Volume(target));
        pulse.set_sink_volume(state.index, &state.channels)?;
    } else {
        return Ok(state);
    }
    Ok(pulse.read(mic)?)
}

fn volume_target(current: u32, up: bool) -> u32 {
    let step = (f64::from(Volume::NORMAL.0) / 100.0).round() as u32;
    if up {
        if current >= Volume::NORMAL.0 {
            current
        } else {
            (current + step).min(Volume::NORMAL.0)
        }
    } else {
        current.saturating_sub(step)
    }
}

fn audio_notification(mic: bool, state: &Device) -> (String, Option<i32>) {
    if mic {
        return (
            if state.muted {
                "󰍭  Microphone Muted"
            } else {
                "󰍬  Microphone Active"
            }
            .into(),
            None,
        );
    }
    if state.muted {
        return ("󰝟  Muted".into(), None);
    }
    // wpctl prints two decimal places before the old script converts to a percentage.
    let percent =
        (f64::from(state.channels.max().0) * 100.0 / f64::from(Volume::NORMAL.0)).round() as i32;
    let icon = if percent < 30 {
        "󰕿"
    } else if percent < 70 {
        "󰖀"
    } else {
        "󰕾"
    };
    (format!("{icon}  Volume: {percent}%"), Some(percent))
}

async fn audio(mic: bool, action: &str) -> Result<()> {
    let name = if mic {
        "hype-mic-control.lock"
    } else {
        "hype-volume-control.lock"
    };
    // Wait for the in-flight PulseAudio update so repeated key presses are applied in full.
    let lock = runtime::lock(name, Duration::from_secs(4))?
        .ok_or_else(|| err("Another audio change is still running"))?;
    let state = match change_audio(mic, action) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("{error}");
            return Ok(());
        }
    };
    drop(lock);
    let (title, progress) = audio_notification(mic, &state);
    let _ = notify::send(
        if mic { 2003 } else { 2001 },
        &title,
        "",
        progress,
        if mic { "" } else { "volume" },
    )
    .await;
    Ok(())
}

fn profile_label(profile: &str) -> (&str, &str) {
    match profile {
        "performance" => ("󰓅", "Performance"),
        "power-saver" => ("󰾆", "Power Saver"),
        "balanced" => ("󰾅", "Balanced"),
        other => ("󰾅", other),
    }
}

async fn set_profile(connection: &gio::DBusConnection, profile: &str) -> Result<()> {
    bus::set_property(
        connection,
        PROFILES,
        PROFILES_PATH,
        PROFILES,
        "ActiveProfile",
        &profile.to_variant(),
    )
    .await
}

async fn power_profile(action: &str) -> Result<()> {
    let connection = bus::system().await?;
    let (profile, cycle_lock) = match action {
        "menu" => {
            let order = ["performance", "balanced", "power-saver"];
            let choices = order.map(|profile| {
                let (icon, name) = profile_label(profile);
                format!("{icon} {name}")
            });
            let selected = dialog::choose("Power Profile", &choices).await?;
            (selected.map(|index| order[index]), None)
        }
        "cycle" => {
            let order = PROFILE_NAMES;
            let lock = runtime::lock("hype-power-profile-cycle.lock", Duration::from_secs(5))?
                .ok_or_else(|| err("Power profile change is busy"))?;
            let active = active_profile(&connection).await?;
            let available = read_available_profiles(&connection).await?;
            let start = order
                .iter()
                .position(|&name| name == active)
                .map_or(0, |index| index + 1);
            let next = order[start..]
                .iter()
                .chain(&order[..start])
                .copied()
                .find(|&name| name != active && available.iter().any(|profile| profile == name));
            (next, Some(lock))
        }
        _ => {
            match active_profile(&connection).await {
                Ok(profile) if !profile.is_empty() => println!(
                    "{}",
                    json!({"text":"", "tooltip":format!("Power Profile: {profile}"), "class":profile,
                "percentage":match profile.as_str() { "performance" => 100, "power-saver" => 0, _ => 50 }})
                ),
                _ => println!(
                    "{}",
                    json!({"text":"", "tooltip":"power profile unavailable", "class":"missing", "percentage":50})
                ),
            }
            return Ok(());
        }
    };
    if let Some(profile) = profile {
        let result = set_profile(&connection, profile).await;
        drop(cycle_lock);
        match result {
            Ok(()) => {
                let (icon, name) = profile_label(profile);
                let _ =
                    notify::send(2004, &format!("{icon}  Power Mode: {name}"), "", None, "").await;
            }
            Err(error) => {
                let _ = notify::send(
                    2004,
                    "Could not set Power Mode",
                    &error.to_string(),
                    None,
                    "",
                )
                .await;
                if action == "cycle" {
                    return Err(error);
                }
            }
        }
    }
    let _ = runtime::signal_waybar(16);
    Ok(())
}

#[derive(Debug, Default, PartialEq)]
struct PowerState {
    on_battery: Option<bool>,
    present: bool,
    kind: u32,
    percentage: Option<f64>,
    active: String,
    available: Vec<String>,
}

impl PowerState {
    fn policy(&self) -> Option<&'static str> {
        if !self.present || self.kind != 2 {
            return None;
        }
        match self.on_battery {
            Some(false) => Some("performance"),
            Some(true) => self
                .percentage
                .filter(|p| p.is_finite() && *p >= 0.0 && p.floor() <= 100.0)
                .map(|p| if p < 30.0 { "power-saver" } else { "balanced" }),
            None => None,
        }
    }
    fn target(&self) -> Option<&'static str> {
        [self.policy()?, "balanced", "power-saver"]
            .into_iter()
            .find(|name| self.available.iter().any(|p| p == name))
    }
    fn key(&self) -> Option<(String, Vec<String>)> {
        self.target()?;
        Some((self.policy()?.into(), self.available.clone()))
    }
}

fn available_profiles(profiles: &[HashMap<String, glib::Variant>]) -> Vec<String> {
    PROFILE_NAMES
        .into_iter()
        .filter(|name| {
            profiles
                .iter()
                .any(|p| p.get("Profile").and_then(|v| v.str()) == Some(name))
        })
        .map(str::to_owned)
        .collect()
}

async fn active_profile(connection: &gio::DBusConnection) -> Result<String> {
    bus::property(
        connection,
        PROFILES,
        PROFILES_PATH,
        PROFILES,
        "ActiveProfile",
    )
    .await
}

async fn read_available_profiles(connection: &gio::DBusConnection) -> Result<Vec<String>> {
    bus::property::<Vec<HashMap<String, glib::Variant>>>(
        connection,
        PROFILES,
        PROFILES_PATH,
        PROFILES,
        "Profiles",
    )
    .await
    .map(|profiles| available_profiles(&profiles))
}

async fn read_power(connection: &gio::DBusConnection) -> PowerState {
    let on_battery = bus::property::<bool>(connection, UPOWER, UPOWER_PATH, UPOWER, "OnBattery")
        .await
        .ok();
    let present = bus::property::<bool>(connection, UPOWER, BATTERY, DEVICE, "IsPresent")
        .await
        .unwrap_or(false);
    let kind = bus::property::<u32>(connection, UPOWER, BATTERY, DEVICE, "Type")
        .await
        .unwrap_or(0);
    let percentage = bus::property::<f64>(connection, UPOWER, BATTERY, DEVICE, "Percentage")
        .await
        .ok();
    let active = active_profile(connection).await.unwrap_or_default();
    let available = read_available_profiles(connection)
        .await
        .unwrap_or_default();
    PowerState {
        on_battery,
        present,
        kind,
        percentage,
        active,
        available,
    }
}

#[derive(Default)]
struct AutoPolicy {
    last_key: Option<(String, Vec<String>)>,
    last_active: String,
}
impl AutoPolicy {
    fn refresh(&mut self, state: &PowerState) {
        if state.active != self.last_active {
            self.last_active = state.active.clone();
            if !state.active.is_empty() {
                let _ = runtime::signal_waybar(16);
            }
        }
    }
    fn should_apply(&mut self, state: &PowerState) -> bool {
        let key = state.key();
        if key.is_none() {
            self.last_key = None;
            return false;
        }
        if key == self.last_key {
            return false;
        }
        self.last_key = key;
        state.target().is_some_and(|target| target != state.active)
    }
    async fn apply(&mut self, connection: &gio::DBusConnection) {
        let mut state = read_power(connection).await;
        self.refresh(&state);
        if !self.should_apply(&state) {
            return;
        }
        let mut attempt = 0;
        loop {
            let Some(target) = state.target() else {
                self.last_key = None;
                return;
            };
            if state.active == target {
                return;
            }
            match set_profile(connection, target).await {
                Ok(()) => {
                    println!("Power profile: {target}");
                    return;
                }
                Err(error) => eprintln!("Could not set {target}: {error}"),
            }
            if attempt == 2 {
                return;
            }
            glib::timeout_future(Duration::from_secs(if attempt == 0 { 1 } else { 3 })).await;
            state = read_power(connection).await;
            self.refresh(&state);
            if state.key() != self.last_key {
                // A policy transition starts a fresh retry budget using the latest state.
                self.last_key = state.key();
                attempt = 0;
            } else {
                attempt += 1;
            }
        }
    }
}

fn relevant_properties(parameters: &glib::Variant, expected_interface: &str) -> bool {
    let Some((interface, changed, invalidated)) =
        parameters.get::<(String, HashMap<String, glib::Variant>, Vec<String>)>()
    else {
        return false;
    };
    interface == expected_interface
        && changed.keys().chain(invalidated.iter()).any(|key| {
            [
                "OnBattery",
                "IsPresent",
                "Type",
                "Percentage",
                "ActiveProfile",
                "Profiles",
            ]
            .contains(&key.as_str())
        })
}

async fn auto_power_profile(action: &str) -> Result<()> {
    if action == "--check" {
        let state = read_power(&bus::system().await?).await;
        println!(
            "on_battery={}\npercentage={}\navailable={}\nactive={}\npolicy={}\ntarget={}",
            state.on_battery.map(|b| b.to_string()).unwrap_or_default(),
            state.percentage.map(|p| p.to_string()).unwrap_or_default(),
            state.available.join(" "),
            state.active,
            state.policy().unwrap_or(""),
            state.target().unwrap_or("")
        );
        return Ok(());
    }
    let Some(_lock) = runtime::lock("hype-auto-power-profile.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let connection = bus::system().await?;
    let pending = Arc::new(Mutex::new(gio::Cancellable::new()));
    let restarted = Rc::new(Cell::new(false));
    let mut subscriptions = Vec::new();
    for (service, path, interface) in [
        (UPOWER, UPOWER_PATH, UPOWER),
        (UPOWER, BATTERY, DEVICE),
        (PROFILES, PROFILES_PATH, PROFILES),
    ] {
        let pending = pending.clone();
        subscriptions.push(connection.subscribe_to_signal(
            Some(service),
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            Some(path),
            Some(interface),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if relevant_properties(signal.parameters, interface) {
                    pending.lock().unwrap().cancel();
                }
            },
        ));
    }
    for service in [UPOWER, PROFILES] {
        let pending = pending.clone();
        let restarted = restarted.clone();
        subscriptions.push(connection.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            Some(service),
            gio::DBusSignalFlags::NONE,
            move |_| {
                restarted.set(true);
                pending.lock().unwrap().cancel();
            },
        ));
    }
    // The round trip orders AddMatch before the initial snapshot; changes during reads stay dirty.
    bus::call(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetId",
        None,
        5000,
    )
    .await?;
    let disconnected = pending.clone();
    let handler = connection.connect_closed(move |_, _, _| disconnected.lock().unwrap().cancel());
    let mut policy = AutoPolicy::default();
    while !connection.is_closed() {
        let event = gio::Cancellable::new();
        *pending.lock().unwrap() = event.clone();
        if restarted.replace(false) {
            policy.last_key = None;
        }
        policy.apply(&connection).await;
        if !connection.is_closed() {
            event.future().await;
        }
    }
    connection.disconnect(handler);
    Err(err("System D-Bus disconnected"))
}

pub async fn run(command: &str, args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("");
    match command {
        "temperature" => crate::temperature::run(command, args).await,
        "brightness" => brightness(action).await,
        "volume" => audio(false, action).await,
        "mic" => audio(true, action).await,
        "power-profile" => power_profile(action).await,
        "auto-power-profile" => auto_power_profile(action).await,
        _ => Err(err(format!("Unknown control: {command}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libpulse_binding::volume::ChannelVolumes;
    fn power(on_battery: bool, percentage: f64) -> PowerState {
        PowerState {
            on_battery: Some(on_battery),
            present: true,
            kind: 2,
            percentage: Some(percentage),
            active: "performance".into(),
            available: PROFILE_NAMES.map(str::to_owned).into(),
        }
    }
    #[test]
    fn brightness_rounding_floor_and_ramp_preserve_boundaries() {
        for (max, current, up, target) in [
            (400, 120, true, 124),
            (400, 120, false, 116),
            (255, 100, true, 103),
            (255, 100, false, 97),
            (255, 4, false, 3),
            (400, 399, true, 400),
            (400, 400, true, 400),
            (400, 5, false, 4),
            (400, 4, false, 4),
            (400, 1, false, 1),
            (400, 0, false, 0),
            (1, 0, true, 1),
            (1, 1, false, 1),
        ] {
            assert_eq!(brightness_target(current, max, up), target);
            let ramp = brightness_ramp(current, target);
            assert!(ramp.len() <= 8);
            assert!(
                ramp.iter()
                    .all(|n| (current.min(target)..=current.max(target)).contains(n))
            );
            assert!(ramp.windows(2).all(|pair| if up {
                pair[0] < pair[1]
            } else {
                pair[0] > pair[1]
            }));
            assert_eq!(ramp.last().copied().unwrap_or(current), target);
        }
        assert_eq!(
            brightness_ramp(500, 510),
            [501, 502, 503, 505, 506, 507, 508, 510]
        );
    }
    #[test]
    fn volume_changes_one_percent_and_preserves_mute_notifications() {
        assert_eq!(volume_target(Volume::NORMAL.0, true), Volume::NORMAL.0);
        let boosted = Volume::NORMAL.0 * 3 / 2;
        assert_eq!(volume_target(boosted, true), boosted);
        assert_eq!(volume_target(Volume::NORMAL.0 - 1, true), Volume::NORMAL.0);
        assert_eq!(volume_target(0, false), 0);
        let mut channels = ChannelVolumes::default();
        channels.set(
            2,
            Volume((f64::from(Volume::NORMAL.0) * 0.65).round() as u32),
        );
        let mut state = Device {
            index: 1,
            channels,
            muted: false,
        };
        assert_eq!(audio_notification(false, &state).1, Some(65));
        assert!(audio_notification(false, &state).0.contains("65%"));
        state.muted = true;
        assert_eq!(audio_notification(false, &state), ("󰝟  Muted".into(), None));
        assert!(
            audio_notification(true, &state)
                .0
                .contains("Microphone Muted")
        );
        let target = volume_target(channels.max().0, true);
        assert_eq!(
            (f64::from(target) * 100.0 / f64::from(Volume::NORMAL.0)).round(),
            66.0
        );
    }
    #[test]
    fn power_policy_boundary_invalid_values_and_ac_precedence() {
        for (battery, percentage, expected) in [
            (false, 0.0, Some("performance")),
            (false, f64::NAN, Some("performance")),
            (true, 0.0, Some("power-saver")),
            (true, 29.999, Some("power-saver")),
            (true, 30.0, Some("balanced")),
            (true, 100.0, Some("balanced")),
            (true, -1.0, None),
            (true, 101.0, None),
            (true, f64::NAN, None),
            (true, f64::INFINITY, None),
        ] {
            assert_eq!(power(battery, percentage).policy(), expected);
        }
        let mut state = power(false, 10.0);
        state.present = false;
        assert_eq!(state.target(), None);
        state.present = true;
        state.kind = 3;
        assert_eq!(state.target(), None);
    }
    #[test]
    fn automatic_policy_preserves_manual_selection_until_transition() {
        let mut policy = AutoPolicy::default();
        let mut state = power(true, 70.0);
        assert!(policy.should_apply(&state));
        state.active = "performance".into();
        state.percentage = Some(50.0);
        assert!(!policy.should_apply(&state));
        state.percentage = Some(29.0);
        assert!(policy.should_apply(&state));
        assert!(!policy.should_apply(&state));
        state.present = false;
        assert!(!policy.should_apply(&state));
        state.present = true;
        assert!(policy.should_apply(&state));
    }
    #[test]
    fn fallback_and_service_restart_reapply_policy() {
        let mut state = power(false, 10.0);
        state.active = "power-saver".into();
        state.available = vec!["power-saver".into(), "balanced".into()];
        assert_eq!(state.target(), Some("balanced"));
        let mut policy = AutoPolicy::default();
        assert!(policy.should_apply(&state));
        assert!(!policy.should_apply(&state));
        state.available.push("performance".into());
        assert_eq!(state.target(), Some("performance"));
        assert!(policy.should_apply(&state));
        policy.last_key = None;
        assert!(policy.should_apply(&state));
    }
    #[test]
    fn profile_order_is_canonical_and_unrelated_dbus_signals_are_ignored() {
        let profiles = ["performance", "power-saver", "balanced", "balanced"]
            .map(|p| HashMap::from([("Profile".into(), p.to_variant())]));
        assert_eq!(available_profiles(&profiles), PROFILE_NAMES);
        let params = (
            DEVICE,
            HashMap::from([("EnergyRate", 1.0.to_variant())]),
            Vec::<String>::new(),
        )
            .to_variant();
        assert!(!relevant_properties(&params, DEVICE));
        let params = (
            DEVICE,
            HashMap::<String, glib::Variant>::new(),
            vec!["Percentage"],
        )
            .to_variant();
        assert!(relevant_properties(&params, DEVICE));
        assert!(!relevant_properties(&params, PROFILES));
    }
}
