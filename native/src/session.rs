use crate::{
    Result,
    bus::{self, LOGIN, LOGIN_MANAGER, LOGIN_PATH},
    caffeine, err, ipc, notify, runtime,
};
use gtk::{gio, glib, prelude::*};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Single,
    Double,
}
fn next_phase(previous: &str, now: u128) -> Option<Phase> {
    let fields: Vec<&str> = previous.split_whitespace().collect();
    if fields.len() == 2
        && fields[0].len() <= 18
        && fields[0].bytes().all(|b| b.is_ascii_digit())
        && let Ok(timestamp) = fields[0].parse::<u128>()
        && now
            .checked_sub(timestamp)
            .is_some_and(|elapsed| elapsed <= 400_000)
    {
        match fields[1] {
            "single" => return Some(Phase::Double),
            "double" => return None,
            _ => {}
        }
    }
    Some(Phase::Single)
}
fn outputs_match(outputs: &Value, saved: &[String], on: bool) -> bool {
    let Some(outputs) = outputs.as_object() else {
        return false;
    };
    !saved.is_empty()
        && saved.iter().all(|name| match outputs.get(name) {
            None => true,
            Some(output) => output
                .get("current_mode")
                .is_some_and(|mode| mode.is_null() != on),
        })
        && (!on
            || outputs.values().any(|output| {
                output
                    .get("current_mode")
                    .is_some_and(|mode| !mode.is_null())
            }))
}
fn ready(hint: bool, locker: bool) -> bool {
    hint && locker
}

fn locker_running() -> bool {
    runtime::named_processes("hyprlock").next().is_some()
}

async fn session_path(conn: &gio::DBusConnection) -> Result<String> {
    let id = std::env::var("XDG_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| err("No XDG_SESSION_ID; cannot confirm session lock"))?;
    let reply = bus::call(
        conn,
        LOGIN,
        LOGIN_PATH,
        LOGIN_MANAGER,
        "GetSession",
        Some(&(id,).to_variant()),
        2_000,
    )
    .await?;
    reply
        .try_child_value(0)
        .and_then(|v| v.get::<glib::variant::ObjectPath>())
        .map(|p| p.to_string())
        .ok_or_else(|| err("logind returned an invalid session path"))
}
async fn locked_hint(conn: &gio::DBusConnection) -> Result<bool> {
    bus::property(
        conn,
        LOGIN,
        &session_path(conn).await?,
        "org.freedesktop.login1.Session",
        "LockedHint",
    )
    .await
}

struct Locker {
    child: Option<Child>,
    lock: Option<runtime::Lock>,
}
impl Drop for Locker {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let lock = self.lock.take();
            std::thread::spawn(move || {
                let _ = child.wait();
                drop(lock);
            });
        }
    }
}
async fn spawn_locker(sleep: bool) -> Result<Option<Locker>> {
    let Some(lock) = runtime::lock("hype-lock-screen.lock", Duration::ZERO)? else {
        return Ok(None);
    };
    if locker_running() {
        return Ok(None);
    }
    let _ = crate::wallpaper::run("wallpaper", &["ensure".into()]).await;
    let mut command = Command::new("hyprlock");
    command.args(["--grace", "0"]);
    if sleep {
        command.args(["--immediate-render", "--no-fade-in"]);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Some(Locker {
        child: Some(child),
        lock: Some(lock),
    }))
}

async fn ensure_locked(conn: &gio::DBusConnection) -> Result<()> {
    if ready(locked_hint(conn).await?, locker_running()) {
        return Ok(());
    }
    let mut spawned = spawn_locker(true).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ready(locked_hint(conn).await?, locker_running()) {
            return Ok(());
        }
        if let Some(locker) = &mut spawned
            && locker.child.as_mut().unwrap().try_wait()?.is_some()
        {
            return Err(err(
                "Password locker exited before session locking was confirmed",
            ));
        }
        if Instant::now() >= deadline {
            return Err(err("Could not confirm the session lock before sleep"));
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
}

async fn lock(sleep: bool, dim: bool) -> Result<()> {
    let _dim_lock = if dim {
        let Some(lock) = runtime::lock("lock-dim.lock", Duration::ZERO)? else {
            return Ok(());
        };
        Some(lock)
    } else {
        None
    };
    let Some(mut locker) = spawn_locker(sleep).await? else {
        return Ok(());
    };
    let mut exit = glib::child_watch_future(glib::Pid(locker.child.as_ref().unwrap().id() as i32));
    let mut exited = None;
    if dim {
        exited = glib::future_with_timeout(Duration::from_secs(30), &mut exit)
            .await
            .ok();
        if exited.is_none() {
            let conn = bus::system().await?;
            if ready(locked_hint(&conn).await.unwrap_or(false), locker_running()) {
                ipc::action(json!({"PowerOffMonitors":{}}))?;
            }
        }
    }
    let (_, status) = match exited {
        Some(exited) => exited,
        None => exit.await,
    };
    // GLib reaped the child, so Locker::drop must not wait on a reusable PID.
    locker.child.take();
    let status = std::process::ExitStatus::from_raw(status);
    if status.success() {
        Ok(())
    } else {
        Err(err(format!("Password locker exited with {status}")))
    }
}

trait DisplayBackend {
    fn outputs(&self) -> Result<Value>;
    fn set_output(&self, name: &str, on: bool) -> Result<()>;
    fn power_on(&self) -> Result<()>;
    async fn confirm_locked(&self) -> Result<()>;
    async fn read_hint(&self) -> Result<bool>;
}
struct Desktop(gio::DBusConnection);
impl DisplayBackend for Desktop {
    fn outputs(&self) -> Result<Value> {
        let reply = ipc::request(json!("Outputs"))?;
        let outputs = reply.get("Outputs").unwrap_or(&reply);
        if !outputs.is_object() {
            return Err(err("Invalid output response from Niri"));
        }
        Ok(outputs.clone())
    }
    fn set_output(&self, name: &str, on: bool) -> Result<()> {
        let result =
            ipc::request(json!({"Output":{"output":name,"action":if on { "On" } else { "Off" }}}))?;
        if result.get("OutputConfigChanged").and_then(Value::as_str) == Some("OutputWasMissing") {
            return Err(err(format!("Output {name} disappeared")));
        }
        Ok(())
    }
    fn power_on(&self) -> Result<()> {
        ipc::action(json!({"PowerOnMonitors":{}}))
    }
    async fn confirm_locked(&self) -> Result<()> {
        ensure_locked(&self.0).await
    }
    async fn read_hint(&self) -> Result<bool> {
        locked_hint(&self.0).await
    }
}
async fn wait_outputs(backend: &impl DisplayBackend, saved: &[String], on: bool) -> Result<()> {
    for _ in 0..50 {
        if outputs_match(&backend.outputs()?, saved, on) {
            return Ok(());
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    Err(err(if on {
        "Could not confirm display restore"
    } else {
        "Could not confirm display blackout"
    }))
}
async fn restore_outputs(backend: &impl DisplayBackend, saved: &[String]) -> Result<()> {
    backend.confirm_locked().await?;
    let connected = backend.outputs()?;
    let mut failed = false;
    for name in saved {
        if connected.get(name).is_some() && backend.set_output(name, true).is_err() {
            failed = true;
        }
    }
    if backend.power_on().is_err() {
        failed = true;
    }
    if wait_outputs(backend, saved, true).await.is_err() {
        failed = true;
    }
    if failed {
        return Err(err(
            "Could not restore all displays; saved outputs retained for retry",
        ));
    }
    Ok(())
}
fn snapshot(path: &Path) -> Result<Vec<String>> {
    let names: Vec<String> = fs::read_to_string(path)?
        .lines()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if names.is_empty()
        || names
            .iter()
            .any(|name| name.len() > 256 || name.chars().any(char::is_control))
    {
        return Err(err("Invalid saved display snapshot"));
    }
    Ok(names)
}

async fn display(backend: &impl DisplayBackend, state: &Path, resume: bool) -> Result<()> {
    if state.exists() || resume {
        if state.exists() {
            let saved = snapshot(state)?;
            restore_outputs(backend, &saved).await?;
            fs::remove_file(state)?;
        } else {
            backend.confirm_locked().await?;
            backend.power_on()?;
        }
        return Ok(());
    }
    backend.read_hint().await?;
    let outputs = backend.outputs()?;
    let saved: Vec<String> = outputs
        .as_object()
        .ok_or_else(|| err("Invalid Niri outputs"))?
        .iter()
        .filter(|(_, output)| {
            output
                .get("current_mode")
                .is_some_and(|mode| !mode.is_null())
        })
        .map(|(name, _)| name.clone())
        .collect();
    if saved.is_empty() {
        return Err(err("No enabled displays could be saved"));
    }
    runtime::atomic_write(state, (saved.join("\n") + "\n").as_bytes())?;
    let mut failed = false;
    for name in &saved {
        if backend.set_output(name, false).is_err() {
            failed = true;
            break;
        }
    }
    if failed || wait_outputs(backend, &saved, false).await.is_err() {
        // Recovery never exposes the desktop before the password lock is confirmed.
        if restore_outputs(backend, &saved).await.is_ok() {
            fs::remove_file(state)?;
        }
        return Err(err(
            "Could not turn off every display; use Copilot again to retry",
        ));
    }
    glib::timeout_future(Duration::from_secs(1)).await;
    backend.confirm_locked().await.map_err(|_| {
        err("Could not confirm the session lock; displays remain off; use Copilot again to retry")
    })
}

async fn copilot_display(mode: &str, token: Option<&str>) -> Result<()> {
    let Some(_lock) = runtime::lock("hype-copilot-displays.lock", Duration::from_secs(20))? else {
        return Err(err("Display shortcut is busy"));
    };
    let dir = runtime::dir()?;
    {
        let Some(_gesture) = runtime::lock("hype-copilot.lock", Duration::from_secs(2))? else {
            return Err(err("Copilot shortcut is busy"));
        };
        let path = dir.join("hype-copilot.state");
        if mode == "resume" {
            let _ = fs::remove_file(path);
        } else if let Some(token) = token
            && fs::read_to_string(path).ok().as_deref().map(str::trim)
                != Some(format!("{token} double").as_str())
        {
            return Ok(());
        }
    }
    let backend = Desktop(bus::system().await?);
    let result = display(
        &backend,
        &dir.join("hype-copilot-displays.state"),
        mode == "resume",
    )
    .await;
    if let Err(error) = &result {
        let _ = notify::send(
            0,
            "Copilot display shortcut",
            &error.to_string(),
            None,
            "device",
        )
        .await;
    }
    result
}

async fn copilot_shortcut() -> Result<()> {
    let dir = runtime::dir()?;
    let state = dir.join("hype-copilot.state");
    let token = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros();
    let phase;
    {
        let Some(_lock) = runtime::lock("hype-copilot.lock", Duration::from_secs(2))? else {
            return Err(err("Copilot shortcut is busy"));
        };
        let Some(next) = next_phase(&fs::read_to_string(&state).unwrap_or_default(), token) else {
            return Ok(());
        };
        phase = next;
        runtime::atomic_write(
            &state,
            format!(
                "{token} {}\n",
                if phase == Phase::Double {
                    "double"
                } else {
                    "single"
                }
            )
            .as_bytes(),
        )?;
    }
    glib::timeout_future(Duration::from_millis(400)).await;
    let token = token.to_string();
    {
        let Some(_lock) = runtime::lock("hype-copilot.lock", Duration::from_secs(2))? else {
            return Err(err("Copilot shortcut is busy"));
        };
        let expected = format!(
            "{token} {}",
            if phase == Phase::Double {
                "double"
            } else {
                "single"
            }
        );
        if fs::read_to_string(&state).ok().as_deref().map(str::trim) != Some(expected.as_str()) {
            return Ok(());
        }
        if phase == Phase::Single {
            fs::remove_file(state)?;
            if dir.join("hype-copilot-displays.state").exists() {
                return Ok(());
            }
            let Ok(conn) = bus::system().await else {
                return Ok(());
            };
            if !matches!(locked_hint(&conn).await, Ok(false)) {
                return Ok(());
            }
            crate::launcher::launch_copilot_editor().await?;
            return Ok(());
        }
    }
    copilot_display("toggle", Some(&token)).await
}

async fn watch_sleep() -> Result<()> {
    use std::{
        cell::RefCell,
        collections::VecDeque,
        rc::Rc,
        sync::{Arc, Mutex},
    };
    // The old swayidle hook holds a different lock until this watcher is ready.
    let Some(_lock) = runtime::lock("hype-sleep-watch.lock", Duration::ZERO)? else {
        return Ok(());
    };
    let conn = bus::system().await?;
    let pending = Rc::new(RefCell::new(VecDeque::new()));
    let wake = Arc::new(Mutex::new(gio::Cancellable::new()));
    let events = pending.clone();
    let queued = wake.clone();
    let _subscription = conn.subscribe_to_signal(
        Some(LOGIN),
        Some(LOGIN_MANAGER),
        Some("PrepareForSleep"),
        Some(LOGIN_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some((start,)) = signal.parameters.get::<(bool,)>() {
                events.borrow_mut().push_back(start);
                queued.lock().unwrap().cancel();
            }
        },
    );
    let owner_changed = Rc::new(std::cell::Cell::new(false));
    let changed = owner_changed.clone();
    let restarted = wake.clone();
    let _owner = conn.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(LOGIN),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if signal
                .parameters
                .get::<(String, String, String)>()
                .is_some_and(|(_, old, _)| !old.is_empty())
            {
                changed.set(true);
                restarted.lock().unwrap().cancel();
            }
        },
    );
    let mut inhibitor = Some(
        caffeine::inhibit(
            &conn,
            "sleep",
            "Hype session lock",
            "Confirm password lock before sleep",
            "delay",
        )
        .await?,
    );
    let disconnected = wake.clone();
    let _closed = conn.connect_closed(move |_, _, _| disconnected.lock().unwrap().cancel());
    crate::watchers::ready(crate::watchers::Watcher::Sleep)?;
    loop {
        let event = gio::Cancellable::new();
        *wake.lock().unwrap() = event.clone();
        if conn.is_closed() {
            return Err(err("System bus disconnected while watching sleep"));
        }
        if owner_changed.get() {
            inhibitor.take();
            match caffeine::inhibit(
                &conn,
                "sleep",
                "Hype session lock",
                "Confirm password lock before sleep",
                "delay",
            )
            .await
            {
                Ok(fd) => {
                    inhibitor = Some(fd);
                    owner_changed.set(false);
                }
                Err(error) => {
                    eprintln!("Could not renew logind sleep inhibitor: {error}");
                    glib::timeout_future(Duration::from_secs(1)).await;
                    continue;
                }
            }
        }
        let next = pending.borrow_mut().pop_front();
        let Some(start) = next else {
            event.future().await;
            continue;
        };
        if start {
            let prepared = ensure_locked(&conn).await.and_then(|()| {
                // Clear the scanout before suspend so resume starts with a fresh frame.
                ipc::action(json!({"PowerOffMonitors":{}}))
            });
            match prepared {
                Ok(()) => {
                    inhibitor.take();
                }
                Err(error) => {
                    eprintln!("{error}; retaining sleep delay inhibitor until resume")
                }
            }
        } else {
            if inhibitor.is_none() {
                match caffeine::inhibit(
                    &conn,
                    "sleep",
                    "Hype session lock",
                    "Confirm password lock before sleep",
                    "delay",
                )
                .await
                {
                    Ok(fd) => inhibitor = Some(fd),
                    Err(error) => {
                        eprintln!("Could not renew logind sleep inhibitor: {error}");
                        owner_changed.set(true);
                    }
                }
            }
            if let Err(error) = copilot_display("resume", None).await {
                eprintln!("{error}");
            }
        }
    }
}

pub async fn run(command: &str, args: &[String]) -> Result<()> {
    match command {
        "monitors" if args == ["on"] => ipc::action(json!({"PowerOnMonitors": {}})),
        "monitors" if args == ["off"] => ipc::action(json!({"PowerOffMonitors": {}})),
        "prepare-sleep" if args == ["--watch"] => watch_sleep().await,
        "prepare-sleep" if args.is_empty() => ensure_locked(&bus::system().await?).await,
        "suspend-now" if args.is_empty() => {
            let conn = bus::system().await?;
            ensure_locked(&conn).await?;
            bus::call(
                &conn,
                LOGIN,
                LOGIN_PATH,
                LOGIN_MANAGER,
                "Suspend",
                Some(&(true,).to_variant()),
                30_000,
            )
            .await?;
            Ok(())
        }
        "lock" | "lock-screen" if args.is_empty() => lock(false, false).await,
        "lock" if args == ["sleep"] => lock(true, false).await,
        "lock-dim" if args.is_empty() => lock(false, true).await,
        "copilot-shortcut" if args.is_empty() => copilot_shortcut().await,
        "copilot-display" if args.is_empty() || args == ["toggle"] || args == ["resume"] => {
            let token = std::env::var("HYPE_COPILOT_TOKEN").ok();
            copilot_display(
                args.first().map(String::as_str).unwrap_or("toggle"),
                token.as_deref(),
            )
            .await
        }
        _ => Err(err(format!("Invalid arguments for {command}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_readiness_requires_hint_and_live_locker() {
        assert!(ready(true, true));
        assert!(!ready(true, false));
        assert!(!ready(false, true));
        assert!(!ready(false, false));
    }
    #[test]
    fn copilot_window_deduplicates_double_and_triple_presses() {
        assert_eq!(next_phase("100 single", 400_100), Some(Phase::Double));
        assert_eq!(next_phase("100 double", 400_100), None);
        assert_eq!(next_phase("100 double", 400_101), Some(Phase::Single));
        assert_eq!(next_phase("500 single", 499), Some(Phase::Single));
        for bad in [
            "",
            "broken state",
            "100 unknown",
            "1 single extra",
            "-1 single",
        ] {
            assert_eq!(next_phase(bad, 500), Some(Phase::Single));
        }
    }
    #[test]
    fn output_confirmation_handles_disconnect_but_requires_visible_restore() {
        let saved = vec!["eDP-1".into(), "DP-1".into()];
        assert!(outputs_match(
            &json!({"eDP-1":{"current_mode":0}}),
            &saved,
            true
        ));
        assert!(!outputs_match(&json!({}), &saved, true));
        assert!(!outputs_match(
            &json!({"eDP-1":{"current_mode":null}}),
            &saved,
            true
        ));
        assert!(outputs_match(
            &json!({"eDP-1":{"current_mode":null}}),
            &saved,
            false
        ));
        assert!(!outputs_match(
            &json!({"eDP-1":{"current_mode":0}}),
            &saved,
            false
        ));
        assert!(!outputs_match(&json!({"eDP-1":{}}), &saved, false));
        assert!(!outputs_match(&json!({}), &[], false));
    }
    struct FakeDesktop {
        outputs: std::cell::RefCell<Value>,
        events: std::cell::RefCell<Vec<String>>,
        lock_ready: bool,
        fail_hint: bool,
        fail_off: bool,
        fail_power: bool,
    }
    impl FakeDesktop {
        fn new(lock_ready: bool) -> Self {
            Self {
                outputs: std::cell::RefCell::new(
                    json!({"eDP-1":{"current_mode":null}, "DP-1":{"current_mode":null}}),
                ),
                events: Default::default(),
                lock_ready,
                fail_hint: false,
                fail_off: false,
                fail_power: false,
            }
        }
    }
    impl DisplayBackend for FakeDesktop {
        fn outputs(&self) -> Result<Value> {
            Ok(self.outputs.borrow().clone())
        }
        fn set_output(&self, name: &str, on: bool) -> Result<()> {
            self.events.borrow_mut().push(format!("{name} {on}"));
            if !on && self.fail_off {
                return Err(err("output apply failed"));
            }
            self.outputs.borrow_mut()[name]["current_mode"] =
                if on { json!(0) } else { Value::Null };
            Ok(())
        }
        fn power_on(&self) -> Result<()> {
            self.events.borrow_mut().push("power on".into());
            if self.fail_power {
                return Err(err("power-on failed"));
            }
            Ok(())
        }
        async fn confirm_locked(&self) -> Result<()> {
            self.events.borrow_mut().push("confirm lock".into());
            if self.lock_ready {
                Ok(())
            } else {
                Err(err("lock not confirmed"))
            }
        }
        async fn read_hint(&self) -> Result<bool> {
            self.events.borrow_mut().push("read hint".into());
            if self.fail_hint {
                Err(err("session unreadable"))
            } else {
                Ok(false)
            }
        }
    }
    #[test]
    fn saved_outputs_are_never_restored_before_lock_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        fs::write(&state, "eDP-1\nDP-1\n").unwrap();
        let desktop = FakeDesktop::new(false);
        assert!(
            glib::MainContext::new()
                .block_on(display(&desktop, &state, true))
                .is_err()
        );
        assert_eq!(*desktop.events.borrow(), ["confirm lock"]);
        assert_eq!(fs::read_to_string(state).unwrap(), "eDP-1\nDP-1\n");
    }
    #[test]
    fn successful_restore_removes_snapshot_after_lock_and_output_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        fs::write(&state, "eDP-1\nDP-1\n").unwrap();
        let desktop = FakeDesktop::new(true);
        glib::MainContext::new()
            .block_on(display(&desktop, &state, false))
            .unwrap();
        assert_eq!(
            *desktop.events.borrow(),
            ["confirm lock", "eDP-1 true", "DP-1 true", "power on"]
        );
        assert!(!state.exists());
    }
    #[test]
    fn failed_restore_keeps_snapshot_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        fs::write(&state, "eDP-1\nDP-1\n").unwrap();
        let mut desktop = FakeDesktop::new(true);
        desktop.fail_power = true;
        assert!(
            glib::MainContext::new()
                .block_on(display(&desktop, &state, true))
                .is_err()
        );
        assert!(state.exists());
    }
    #[test]
    fn resume_without_snapshot_confirms_lock_before_dpms_wake() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        let desktop = FakeDesktop::new(true);
        glib::MainContext::new()
            .block_on(display(&desktop, &state, true))
            .unwrap();
        assert_eq!(*desktop.events.borrow(), ["confirm lock", "power on"]);
        assert!(!state.exists());
    }
    #[test]
    fn partial_blackout_cannot_rollback_until_password_lock_is_confirmed() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        let mut desktop = FakeDesktop::new(false);
        *desktop.outputs.borrow_mut() =
            json!({"eDP-1":{"current_mode":0}, "DP-1":{"current_mode":0}});
        desktop.fail_off = true;
        assert!(
            glib::MainContext::new()
                .block_on(display(&desktop, &state, false))
                .is_err()
        );
        assert!(state.exists());
        assert_eq!(
            *desktop.events.borrow(),
            ["read hint", "DP-1 false", "confirm lock"]
        );
    }
    #[test]
    fn unreadable_session_prevents_blackout_and_snapshot_creation() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("saved");
        let mut desktop = FakeDesktop::new(false);
        desktop.fail_hint = true;
        assert!(
            glib::MainContext::new()
                .block_on(display(&desktop, &state, false))
                .is_err()
        );
        assert_eq!(*desktop.events.borrow(), ["read hint"]);
        assert!(!state.exists());
    }
}
