//! Media-key control over MPRIS, without a playerctl subprocess.
use crate::{Result, bus, err};
use gtk::{glib, prelude::*};
use std::collections::HashMap;

#[path = "mpris.rs"]
mod mpris;

const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PATH: &str = "/org/mpris/MediaPlayer2";
type Properties = HashMap<String, glib::Variant>;

pub async fn run(args: &[String]) -> Result<()> {
    let method = match args {
        [action] if action == "play-pause" => "PlayPause",
        [action] if action == "next" => "Next",
        [action] if action == "previous" => "Previous",
        _ => return Err(err("Usage: hype-control media {play-pause|next|previous}")),
    };
    let connection = bus::session().await?;
    let names = bus::call(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListNames",
        None,
        1500,
    )
    .await?
    .get::<(Vec<String>,)>()
    .ok_or_else(|| err("Invalid D-Bus player list"))?
    .0;
    let mut names: Vec<_> = names
        .into_iter()
        .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
        .collect();
    names.sort();
    let mut candidates = Vec::new();
    for name in names.into_iter().take(64) {
        // A player may quit between discovery, property reads and the final action.
        let Ok(reply) = bus::call(
            &connection,
            &name,
            PATH,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            Some(&(PLAYER,).to_variant()),
            1000,
        )
        .await
        else {
            continue;
        };
        if let Some((properties,)) = reply.get::<(Properties,)>()
            && let Some(rank) = mpris::priority(|key| properties.get(key).cloned(), method)
        {
            candidates.push((rank, name));
        }
    }
    candidates.sort();
    for (_, name) in candidates {
        if bus::call(&connection, &name, PATH, PLAYER, method, None, 1500)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    // No live capable player is normal when a media key is pressed on an idle desktop.
    Ok(())
}
