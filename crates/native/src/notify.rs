use crate::{Result, bus};
use gtk::{glib, prelude::*};
use std::collections::HashMap;
pub async fn send(
    id: u32,
    title: &str,
    body: &str,
    progress: Option<i32>,
    category: &str,
) -> Result<()> {
    let connection = bus::session().await?;
    let mut hints = HashMap::<String, glib::Variant>::new();
    hints.insert(
        "x-canonical-private-synchronous".into(),
        category.to_variant(),
    );
    if let Some(value) = progress {
        hints.insert("value".into(), value.to_variant());
    }
    bus::call(
        &connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        Some(
            &(
                "hype-niri",
                id,
                "",
                title,
                body,
                Vec::<String>::new(),
                hints,
                -1_i32,
            )
                .to_variant(),
        ),
        1500,
    )
    .await?;
    Ok(())
}
