use crate::{Result, bus, err, runtime, stealth};
use gtk::{gio, prelude::*};
use std::{fs, path::Path, time::Duration};
async fn state(conn: &gio::DBusConnection) -> Result<String> {
    bus::unit_state(conn, "stealth.service").await
}
async fn change(conn: &gio::DBusConnection, start: bool) -> Result<()> {
    let timeout = Duration::from_secs(if start { 240 } else { 120 });
    let status = bus::unit_change(conn, "stealth.service", start, timeout).await?;
    if status == if start { "active" } else { "inactive" } {
        return Ok(());
    }
    if status == "failed" {
        return Err(err(
            "Stealth service failed; inspect its status before retrying",
        ));
    }
    Err(err("Stealth service operation timed out"))
}
pub async fn root(args: &[String]) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(err(
            "Stealth service management requires elevated privileges",
        ));
    }
    let connection = bus::system().await?;
    match args {
        [action] if action == "manage-start" => {
            if Path::new("/run/stealth/active").exists()
                || Path::new("/run/stealth/state").exists()
                || stealth::firewall_present()?
            {
                return stealth::run(&["status".into()]).await;
            }
            if state(&connection).await? == "active" {
                change(&connection, false).await?;
            }
            change(&connection, true).await?;
            stealth::run(&["status".into()]).await?;
            println!("✓ Stealth enabled");
            Ok(())
        }
        [action] if action == "manage-stop" => {
            stealth::run(&["stop".into()]).await?;
            let current = state(&connection).await?;
            if current != "inactive" && current != "failed" {
                change(&connection, false).await?;
            }
            if state(&connection).await? == "failed" {
                bus::call(
                    &connection,
                    bus::SYSTEMD,
                    bus::SYSTEMD_PATH,
                    bus::SYSTEMD_MANAGER,
                    "ResetFailedUnit",
                    Some(&("stealth.service",).to_variant()),
                    5000,
                )
                .await?;
            }
            println!("✓ Stealth disabled");
            Ok(())
        }
        _ => Err(err("Unknown Stealth service operation")),
    }
}
pub async fn run(args: &[String]) -> Result<()> {
    let action = match args {
        [] => "status",
        [action] if matches!(action.as_str(), "start" | "stop" | "status") => action.as_str(),
        _ => return Err(err("Usage: hype-control stealth {start|stop|status}")),
    };
    let helper = Path::new("/usr/local/bin/stealth");
    if !fs::metadata(helper).is_ok_and(|metadata| metadata.is_file()) {
        return Err(err(
            "Stealth is not installed. Run the optional Stealth installer first",
        ));
    }
    let action = match action {
        "start" => "manage-start",
        "stop" => "manage-stop",
        _ => "status",
    };
    let status = std::process::Command::new("pkexec")
        .arg(helper)
        .arg(action)
        .status()?;
    if action != "status" {
        let _ = runtime::signal_waybar(18);
    }
    if status.success() {
        Ok(())
    } else {
        Err(err("Stealth operation did not complete"))
    }
}
