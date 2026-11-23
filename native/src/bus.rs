use crate::{Result, err};
use gtk::{gio, glib, prelude::*};
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
