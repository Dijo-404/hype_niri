use gtk::glib;

pub fn priority(property: impl Fn(&str) -> Option<glib::Variant>, method: &str) -> Option<u8> {
    let boolean = |key| property(key).and_then(|v| v.get::<bool>()) == Some(true);
    if !boolean("CanControl") {
        return None;
    }
    let status = property("PlaybackStatus");
    let status = status.as_ref().and_then(|v| v.str());
    let capability = match method {
        "Next" => "CanGoNext",
        "Previous" => "CanGoPrevious",
        _ if status == Some("Playing") => "CanPause",
        _ => "CanPlay",
    };
    boolean(capability).then_some(match status {
        Some("Playing") => 0,
        Some("Paused") => 1,
        _ => 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;
    #[test]
    fn priority_requires_control_and_ranks_playing_then_paused() {
        let player = |control: bool, status: &'static str| {
            move |key: &str| match key {
                "CanControl" => Some(control.to_variant()),
                "PlaybackStatus" => Some(status.to_variant()),
                "CanPlay" | "CanPause" => Some(true.to_variant()),
                _ => None,
            }
        };
        assert_eq!(priority(player(true, "Playing"), "PlayPause"), Some(0));
        assert_eq!(priority(player(true, "Paused"), "PlayPause"), Some(1));
        assert_eq!(priority(player(true, "Stopped"), "PlayPause"), Some(2));
        assert_eq!(priority(player(false, "Playing"), "PlayPause"), None);
        assert_eq!(priority(player(true, "Playing"), "Next"), None);
    }
}
