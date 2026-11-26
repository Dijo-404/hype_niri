use crate::{Result, err};
use gtk::{gio, glib, prelude::*};
use std::time::{Duration, Instant};

pub const LOGIN: &str = "org.freedesktop.login1";
pub const LOGIN_MANAGER: &str = "org.freedesktop.login1.Manager";
pub const LOGIN_PATH: &str = "/org/freedesktop/login1";
pub const SYSTEMD: &str = "org.freedesktop.systemd1";
pub const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
pub const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

pub async fn system() -> Result<gio::DBusConnection> {
    Ok(gio::bus_get_future(gio::BusType::System).await?)
}
pub async fn session() -> Result<gio::DBusConnection> {
    Ok(gio::bus_get_future(gio::BusType::Session).await?)
}
pub async fn call(
    connection: &gio::DBusConnection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    args: Option<&glib::Variant>,
    timeout_ms: i32,
) -> Result<glib::Variant> {
    Ok(connection
        .call_future(
            Some(destination),
            path,
            interface,
            method,
            args,
            None,
            gio::DBusCallFlags::NONE,
            timeout_ms,
        )
        .await?)
}
pub async fn property<T: glib::variant::FromVariant>(
    connection: &gio::DBusConnection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Result<T> {
    let reply = call(
        connection,
        destination,
        path,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(&(interface, property).to_variant()),
        5000,
    )
    .await?;
    let (value,) = reply
        .get::<(glib::Variant,)>()
        .ok_or_else(|| err("Invalid D-Bus property reply"))?;
    value
        .get::<T>()
        .ok_or_else(|| err(format!("Invalid D-Bus property type: {property}")))
}
pub async fn set_property(
    connection: &gio::DBusConnection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
    value: &glib::Variant,
) -> Result<()> {
    call(
        connection,
        destination,
        path,
        "org.freedesktop.DBus.Properties",
        "Set",
        Some(&(interface, property, value.clone()).to_variant()),
        5000,
    )
    .await?;
    Ok(())
}
pub async fn unit_state(connection: &gio::DBusConnection, unit: &str) -> Result<String> {
    let reply = call(
        connection,
        SYSTEMD,
        SYSTEMD_PATH,
        SYSTEMD_MANAGER,
        "LoadUnit",
        Some(&(unit,).to_variant()),
        5000,
    )
    .await?;
    let (path,) = reply
        .get::<(glib::variant::ObjectPath,)>()
        .ok_or_else(|| err("Invalid systemd unit path"))?;
    property(
        connection,
        SYSTEMD,
        path.as_str(),
        "org.freedesktop.systemd1.Unit",
        "ActiveState",
    )
    .await
}
pub async fn unit_change(
    connection: &gio::DBusConnection,
    unit: &str,
    start: bool,
    timeout: Duration,
) -> Result<String> {
    call(
        connection,
        SYSTEMD,
        SYSTEMD_PATH,
        SYSTEMD_MANAGER,
        if start { "StartUnit" } else { "StopUnit" },
        Some(&(unit, "replace").to_variant()),
        5000,
    )
    .await?;
    let deadline = Instant::now() + timeout;
    loop {
        let state = unit_state(connection, unit).await?;
        if state == if start { "active" } else { "inactive" }
            || state == "failed"
            || Instant::now() >= deadline
        {
            return Ok(state);
        }
        glib::timeout_future(Duration::from_millis(200)).await;
    }
}
