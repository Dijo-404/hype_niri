use crate::{Result, bus, dialog, err, notify, runtime};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::Cell,
    collections::HashMap,
    io::Write,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub const NM: &str = "org.freedesktop.NetworkManager";
pub const ROOT: &str = "/org/freedesktop/NetworkManager";
pub const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
pub type Properties = HashMap<String, glib::Variant>;
pub type Settings = HashMap<String, Properties>;
pub type ObjectPath = glib::variant::ObjectPath;
pub fn path(value: &str) -> Result<ObjectPath> {
    Ok(ObjectPath::try_from(value)?)
}
pub struct Manager {
    pub connection: gio::DBusConnection,
}
impl Manager {
    pub async fn new() -> Result<Self> {
        Ok(Self {
            connection: bus::system().await?,
        })
    }
    pub async fn call(
        &self,
        object: &str,
        interface: &str,
        method: &str,
        args: Option<&glib::Variant>,
    ) -> Result<glib::Variant> {
        bus::call(&self.connection, NM, object, interface, method, args, 5000).await
    }
    pub async fn property<T: glib::variant::FromVariant>(
        &self,
        object: &str,
        interface: &str,
        key: &str,
    ) -> Result<T> {
        bus::property(&self.connection, NM, object, interface, key).await
    }
    pub async fn profiles(&self) -> Result<Vec<ObjectPath>> {
        self.call(SETTINGS, &format!("{NM}.Settings"), "ListConnections", None)
            .await?
            .get::<(Vec<ObjectPath>,)>()
            .map(|v| v.0)
            .ok_or_else(|| err("Invalid NetworkManager profile list"))
    }
    pub async fn settings(&self, object: &str) -> Result<Settings> {
        self.call(
            object,
            &format!("{NM}.Settings.Connection"),
            "GetSettings",
            None,
        )
        .await?
        .get::<(Settings,)>()
        .map(|v| v.0)
        .ok_or_else(|| err("Invalid NetworkManager settings"))
    }
    pub async fn update(&self, object: &str, settings: &Settings, temporary: bool) -> Result<()> {
        self.call(
            object,
            &format!("{NM}.Settings.Connection"),
            if temporary { "UpdateUnsaved" } else { "Update" },
            Some(&(settings.clone(),).to_variant()),
        )
        .await?;
        Ok(())
    }
    pub async fn add(&self, settings: &Settings, temporary: bool) -> Result<ObjectPath> {
        let reply = self
            .call(
                SETTINGS,
                &format!("{NM}.Settings"),
                if temporary {
                    "AddConnectionUnsaved"
                } else {
                    "AddConnection"
                },
                Some(&(settings.clone(),).to_variant()),
            )
            .await?;
        reply
            .get::<(ObjectPath,)>()
            .map(|v| v.0)
            .ok_or_else(|| err("Invalid new connection path"))
    }
    pub async fn devices(&self) -> Result<Vec<ObjectPath>> {
        self.call(ROOT, NM, "GetDevices", None)
            .await?
            .get::<(Vec<ObjectPath>,)>()
            .map(|v| v.0)
            .ok_or_else(|| err("Invalid device list"))
    }
    pub async fn device(&self, name: &str) -> Result<ObjectPath> {
        self.call(ROOT, NM, "GetDeviceByIpIface", Some(&(name,).to_variant()))
            .await?
            .get::<(ObjectPath,)>()
            .map(|v| v.0)
            .ok_or_else(|| err("Invalid device path"))
    }
    pub async fn activate(&self, profile: &str, device: &str) -> Result<ObjectPath> {
        self.call(
            ROOT,
            NM,
            "ActivateConnection",
            Some(&(path(profile)?, path(device)?, path("/")?).to_variant()),
        )
        .await?
        .get::<(ObjectPath,)>()
        .map(|v| v.0)
        .ok_or_else(|| err("Invalid active connection path"))
    }
    pub async fn deactivate(&self, active: &str) -> Result<()> {
        self.call(
            ROOT,
            NM,
            "DeactivateConnection",
            Some(&(path(active)?,).to_variant()),
        )
        .await?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self
                .property::<u32>(active, &format!("{NM}.Connection.Active"), "State")
                .await
            {
                Ok(4) => return Ok(()),
                Err(error)
                    if error.to_string().contains("UnknownObject")
                        || error.to_string().contains("UnknownMethod") =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(err("Hotspot deactivation timed out"));
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
    }
    pub async fn wait_active(&self, active: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .property::<u32>(active, &format!("{NM}.Connection.Active"), "State")
                .await
            {
                Ok(2) => return Ok(()),
                Ok(3 | 4) => return Err(err("Network activation failed")),
                Err(e) => return Err(e),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(err("Network activation timed out"));
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }
    }
    pub async fn delete(&self, profile: &str) -> Result<()> {
        self.call(
            profile,
            &format!("{NM}.Settings.Connection"),
            "Delete",
            None,
        )
        .await?;
        Ok(())
    }
    pub async fn by_uuid(&self, uuid: &str) -> Result<Option<ObjectPath>> {
        for object in self.profiles().await? {
            if text(&self.settings(object.as_str()).await?, "connection", "uuid") == Some(uuid) {
                return Ok(Some(object));
            }
        }
        Ok(None)
    }
}
pub fn text<'a>(settings: &'a Settings, section: &str, key: &str) -> Option<&'a str> {
    settings.get(section)?.get(key)?.str()
}
fn set<T: glib::variant::ToVariant>(settings: &mut Settings, section: &str, key: &str, value: T) {
    settings
        .entry(section.into())
        .or_default()
        .insert(key.into(), value.to_variant());
}
pub fn valid_password(password: &str) -> bool {
    (password.len() >= 8
        && password.len() <= 63
        && password.bytes().all(|b| (0x20..=0x7e).contains(&b)))
        || (password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn secure_hotspot(settings: &mut Settings, band: &str, channel: u32) {
    set(settings, "802-11-wireless", "band", band);
    set(settings, "802-11-wireless", "channel", channel);
    set(settings, "802-11-wireless-security", "key-mgmt", "wpa-psk");
    set(settings, "802-11-wireless-security", "proto", vec!["rsn"]);
    set(settings, "802-11-wireless-security", "pmf", 2_i32);
    set(
        settings,
        "802-11-wireless-security",
        "pairwise",
        vec!["ccmp"],
    );
    set(settings, "802-11-wireless-security", "group", vec!["ccmp"]);
}
async fn profile(manager: &Manager) -> Result<Option<(ObjectPath, Settings)>> {
    let mut found = None;
    for object in manager.profiles().await? {
        let settings = manager.settings(object.as_str()).await?;
        if text(&settings, "connection", "id") != Some("Hype Hotspot") {
            continue;
        }
        if found.is_some() {
            return Err(err(
                "More than one connection is named Hype Hotspot. Rename duplicates in Network Connections.",
            ));
        }
        if text(&settings, "802-11-wireless", "mode") != Some("ap")
            || text(&settings, "ipv4", "method") != Some("shared")
        {
            return Err(err(
                "The name 'Hype Hotspot' belongs to another connection. Rename it in Network Connections.",
            ));
        }
        found = Some((object, settings));
    }
    Ok(found)
}
async fn active(manager: &Manager, profile: &str) -> Result<Option<ObjectPath>> {
    let list: Vec<ObjectPath> = manager.property(ROOT, NM, "ActiveConnections").await?;
    for item in list {
        let connection: ObjectPath = manager
            .property(
                item.as_str(),
                &format!("{NM}.Connection.Active"),
                "Connection",
            )
            .await?;
        if connection.as_str() == profile {
            return Ok(Some(item));
        }
    }
    Ok(None)
}
async fn adapter(manager: &Manager, preferred: Option<&str>) -> Result<(ObjectPath, bool)> {
    for device in manager.devices().await? {
        if let Some(name) = preferred
            && manager
                .property::<String>(device.as_str(), &format!("{NM}.Device"), "Interface")
                .await?
                != name
        {
            continue;
        }
        if manager
            .property::<u32>(device.as_str(), &format!("{NM}.Device"), "DeviceType")
            .await?
            != 2
        {
            continue;
        }
        let capabilities: u32 = manager
            .property(
                device.as_str(),
                &format!("{NM}.Device.Wireless"),
                "WirelessCapabilities",
            )
            .await?;
        if capabilities & 0x40 != 0 {
            return Ok((device, capabilities & 0x400 != 0));
        }
    }
    Err(err("No Wi-Fi adapter supports hotspot mode."))
}
async fn start(manager: &Manager) -> Result<()> {
    let saved = profile(manager).await?;
    let preferred = saved
        .as_ref()
        .and_then(|(_, settings)| text(settings, "connection", "interface-name"))
        .filter(|name| !name.is_empty());
    let (device, five) = adapter(manager, preferred).await?;
    let (object, mut settings) = if let Some(profile) = saved {
        profile
    } else {
        let mut settings = Settings::new();
        set(&mut settings, "connection", "id", "Hype Hotspot");
        set(&mut settings, "connection", "type", "802-11-wireless");
        set(&mut settings, "connection", "autoconnect", false);
        let interface: String = manager
            .property(device.as_str(), &format!("{NM}.Device"), "Interface")
            .await?;
        set(&mut settings, "connection", "interface-name", interface);
        set(
            &mut settings,
            "802-11-wireless",
            "ssid",
            b"Hype-Niri".to_vec(),
        );
        set(&mut settings, "802-11-wireless", "mode", "ap");
        set(&mut settings, "ipv4", "method", "shared");
        set(&mut settings, "ipv6", "method", "disabled");
        let mut bytes = [0_u8; 12];
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let password = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        set(&mut settings, "802-11-wireless-security", "psk", password);
        secure_hotspot(
            &mut settings,
            if five { "a" } else { "bg" },
            if five { 36 } else { 1 },
        );
        let object = manager.add(&settings, false).await?;
        (object, settings)
    };
    let band = if text(&settings, "802-11-wireless", "band") == Some("bg") {
        "bg"
    } else if five {
        "a"
    } else {
        "bg"
    };
    let channel = if band == "bg" {
        settings
            .get("802-11-wireless")
            .and_then(|p| p.get("channel"))
            .and_then(|v| v.get::<u32>())
            .filter(|v| *v > 0)
            .unwrap_or(1)
    } else {
        36
    };
    let old = settings.to_variant();
    secure_hotspot(&mut settings, band, channel);
    let changed = settings.to_variant() != old;
    if changed {
        manager.update(object.as_str(), &settings, false).await?;
        if let Some(active) = active(manager, object.as_str()).await? {
            manager.deactivate(active.as_str()).await?;
        }
    }
    if active(manager, object.as_str()).await?.is_some() {
        return Ok(());
    }
    let first = manager.activate(object.as_str(), device.as_str()).await;
    let result = match first {
        Ok(ref active) => {
            manager
                .wait_active(active.as_str(), Duration::from_secs(35))
                .await
        }
        Err(e) => Err(e),
    };
    if let Err(error) = result {
        if band != "a" {
            return Err(error);
        }
        if let Some(active) = active(manager, object.as_str()).await? {
            manager.deactivate(active.as_str()).await?;
        }
        secure_hotspot(&mut settings, "bg", 1);
        manager.update(object.as_str(), &settings, false).await?;
        let active = manager.activate(object.as_str(), device.as_str()).await?;
        manager
            .wait_active(active.as_str(), Duration::from_secs(35))
            .await?;
    }
    let _ = notify::send(
        0,
        "Hotspot started",
        "Right-click the hotspot button to show credentials or edit the network.",
        None,
        "hotspot",
    )
    .await;
    Ok(())
}
async fn stop(manager: &Manager) -> Result<()> {
    if let Some((object, _)) = profile(manager).await?
        && let Some(active) = active(manager, object.as_str()).await?
    {
        manager.deactivate(active.as_str()).await?;
        let _ = notify::send(0, "Hotspot stopped", "", None, "hotspot").await;
    }
    Ok(())
}
async fn credentials(manager: &Manager) -> Result<()> {
    let (object, mut settings) = profile(manager)
        .await?
        .ok_or_else(|| err("Start the hotspot once to create its saved profile."))?;
    if active(manager, object.as_str()).await?.is_none() {
        return Err(err("Start the hotspot before showing its credentials."));
    }
    let secrets = manager
        .call(
            object.as_str(),
            &format!("{NM}.Settings.Connection"),
            "GetSecrets",
            Some(&("802-11-wireless-security",).to_variant()),
        )
        .await?
        .get::<(Settings,)>()
        .ok_or_else(|| err("Invalid credentials response"))?
        .0;
    settings.extend(secrets);
    let password = text(&settings, "802-11-wireless-security", "psk")
        .ok_or_else(|| err("Hotspot password unavailable"))?;
    let ssid = settings
        .get("802-11-wireless")
        .and_then(|p| p.get("ssid"))
        .and_then(|v| v.get::<Vec<u8>>())
        .ok_or_else(|| err("Hotspot name unavailable"))?;
    dialog::credentials("Hotspot credentials", std::str::from_utf8(&ssid)?, password)
}
async fn password(manager: &Manager) -> Result<()> {
    let (object, mut settings) = profile(manager)
        .await?
        .ok_or_else(|| err("Start the hotspot once to create its saved profile."))?;
    let Some(password) = dialog::prompt("New hotspot password", true)? else {
        return Ok(());
    };
    if password.is_empty() {
        return Ok(());
    }
    if !valid_password(&password) {
        return Err(err("Use 8–63 ASCII characters or 64 hexadecimal digits."));
    }
    set(&mut settings, "802-11-wireless-security", "psk", password);
    manager
        .update(object.as_str(), &settings, false)
        .await
        .map_err(|_| {
            err("Could not save the hotspot password. Check NetworkManager permissions.")
        })?;
    if let Some(connection) = active(manager, object.as_str()).await? {
        manager.deactivate(connection.as_str()).await?;
        start(manager).await?;
    }
    let _ = notify::send(
        0,
        "Hotspot password changed",
        "Reconnect your devices using the new password.",
        None,
        "hotspot",
    )
    .await;
    Ok(())
}
async fn is_active(manager: &Manager, profile: Option<&ObjectPath>) -> Result<bool> {
    match profile {
        Some(profile) => Ok(active(manager, profile.as_str()).await?.is_some()),
        None => Ok(false),
    }
}

fn status_json(status: Result<bool>) -> serde_json::Value {
    let (class, tooltip) = match status {
        Ok(true) => (
            "active",
            "Hotspot on · Click to stop · Right-click for credentials",
        ),
        Ok(false) => (
            "inactive",
            "Hotspot off · Click to start (may disconnect Wi-Fi) · Right-click for menu",
        ),
        Err(error) => {
            eprintln!("{error}");
            (
                "unavailable",
                "Hotspot unavailable · Open hotspot menu for details",
            )
        }
    };
    serde_json::json!({"text":"󰀂", "class":class, "tooltip":tooltip})
}

async fn status(manager: &Manager) -> Result<bool> {
    let saved = profile(manager).await?;
    is_active(manager, saved.as_ref().map(|(object, _)| object)).await
}

async fn watch(manager: Manager) -> Result<()> {
    let connection = &manager.connection;
    let pending = Arc::new(Mutex::new(gio::Cancellable::new()));
    let profiles_dirty = Rc::new(Cell::new(true));
    let mut subscriptions = Vec::new();
    for (interface, member, path) in [
        (
            "org.freedesktop.NetworkManager.Settings",
            "NewConnection",
            Some(SETTINGS),
        ),
        (
            "org.freedesktop.NetworkManager.Settings",
            "ConnectionRemoved",
            Some(SETTINGS),
        ),
        (
            "org.freedesktop.NetworkManager.Settings.Connection",
            "Updated",
            None,
        ),
        (
            "org.freedesktop.NetworkManager.Settings.Connection",
            "Removed",
            None,
        ),
    ] {
        let profiles_dirty = profiles_dirty.clone();
        let pending = pending.clone();
        subscriptions.push(connection.subscribe_to_signal(
            Some(NM),
            Some(interface),
            Some(member),
            path,
            None,
            gio::DBusSignalFlags::NONE,
            move |_| {
                profiles_dirty.set(true);
                pending.lock().unwrap().cancel();
            },
        ));
    }
    let changed = pending.clone();
    subscriptions.push(connection.subscribe_to_signal(
        Some(NM),
        Some("org.freedesktop.DBus.Properties"),
        Some("PropertiesChanged"),
        Some(ROOT),
        Some(NM),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if signal
                .parameters
                .get::<(String, Properties, Vec<String>)>()
                .is_some_and(|(_, changed, invalidated)| {
                    changed.contains_key("ActiveConnections")
                        || invalidated.iter().any(|key| key == "ActiveConnections")
                })
            {
                changed.lock().unwrap().cancel();
            }
        },
    ));
    let restarted = pending.clone();
    let dirty = profiles_dirty.clone();
    subscriptions.push(connection.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(NM),
        gio::DBusSignalFlags::NONE,
        move |_| {
            dirty.set(true);
            restarted.lock().unwrap().cancel();
        },
    ));
    let disconnected = pending.clone();
    let handler = connection.connect_closed(move |_, _, _| disconnected.lock().unwrap().cancel());
    let result = async {
        let mut saved = None;
        let mut previous = None;
        while !connection.is_closed() {
            let event = gio::Cancellable::new();
            *pending.lock().unwrap() = event.clone();
            let status = async {
                if profiles_dirty.replace(false) {
                    saved = match profile(&manager).await {
                        Ok(profile) => profile.map(|(path, _)| path),
                        Err(error) => {
                            profiles_dirty.set(true);
                            return Err(error);
                        }
                    };
                }
                is_active(&manager, saved.as_ref()).await
            }
            .await;
            let retry = status.is_err();
            let output = status_json(status);
            if previous.as_ref() != Some(&output) {
                let mut out = std::io::stdout().lock();
                writeln!(out, "{output}")?;
                out.flush()?;
                previous = Some(output);
            }
            if retry {
                glib::timeout_future(Duration::from_secs(5)).await;
            } else if !connection.is_closed() {
                event.future().await;
            }
        }
        Ok(())
    }
    .await;
    connection.disconnect(handler);
    result
}

pub async fn watch_status() -> Result<()> {
    watch(Manager::new().await?).await
}

pub async fn run(_command: &str, args: &[String]) -> Result<()> {
    let mut action = args.first().map(String::as_str).unwrap_or("status");
    if action == "watch" {
        return watch_status().await;
    }
    if action == "status" {
        let status = match Manager::new().await {
            Ok(manager) => status(&manager).await,
            Err(error) => Err(error),
        };
        println!("{}", status_json(status));
        return Ok(());
    }
    if action == "menu" {
        let choices = [
            "Start hotspot",
            "Stop hotspot",
            "Show credentials",
            "Change password",
            "Edit hotspot",
        ]
        .map(String::from);
        let Some(choice) = dialog::choose("Hotspot", &choices)? else {
            return Ok(());
        };
        action = ["start", "stop", "credentials", "password", "edit"][choice];
    }
    let result = async {
        let _lock = if matches!(action, "start" | "stop" | "toggle" | "password") {
            let Some(lock) = runtime::lock("hype-hotspot.lock", Duration::ZERO)? else {
                let _ = notify::send(
                    0, "Hotspot busy", "Another hotspot action is still running.", None, "hotspot",
                ).await;
                return Ok(());
            };
            Some(lock)
        } else {
            None
        };
        let manager = Manager::new().await?;
        match action {
            "start" => start(&manager).await?,
            "stop" => stop(&manager).await?,
            "toggle" => {
                if status(&manager).await? {
                    stop(&manager).await?;
                } else {
                    start(&manager).await?;
                }
            }
            "credentials" => credentials(&manager).await?,
            "password" => password(&manager).await?,
            "edit" => {
                let (_, settings) = profile(&manager)
                    .await?
                    .ok_or_else(|| err("Start the hotspot once to create its saved profile."))?;
                let uuid = text(&settings, "connection", "uuid").ok_or_else(|| err("Missing profile UUID"))?;
                std::process::Command::new("nm-connection-editor")
                    .arg(format!("--edit={uuid}"))
                    .spawn()?;
            }
            _ => return Err(err("Usage: hype-control hotspot {status|watch|menu|toggle|start|stop|credentials|password|edit}")),
        }
        if matches!(action, "start" | "stop" | "toggle") {
            let _ = runtime::signal_waybar(17);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = notify::send(0, "Hotspot failed", &error.to_string(), None, "hotspot").await;
        return Err(error);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passwords_enforce_wpa_psk_contract() {
        for p in ["12345678", "eight;\\: words!", &"a".repeat(64)] {
            assert!(valid_password(p));
        }
        for p in ["1234567", "nonasciié", "bad\npass", &"g".repeat(64)] {
            assert!(!valid_password(p));
        }
    }
    #[test]
    fn hotspot_security_preserves_identity_and_enforces_ciphers() {
        let mut s = Settings::new();
        set(&mut s, "connection", "id", "Hype Hotspot");
        secure_hotspot(&mut s, "bg", 1);
        assert_eq!(text(&s, "connection", "id"), Some("Hype Hotspot"));
        assert_eq!(
            text(&s, "802-11-wireless-security", "key-mgmt"),
            Some("wpa-psk")
        );
        assert_eq!(s["802-11-wireless"]["channel"].get::<u32>(), Some(1));
        assert_eq!(
            s["802-11-wireless-security"]["proto"].get::<Vec<String>>(),
            Some(vec!["rsn".into()])
        );
    }
}
