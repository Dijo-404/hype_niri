use crate::{Result, err};
use std::{io::Write, os::fd::AsFd};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};

// Printable evdev codes avoid Chromium interpreting Unicode text as control keys.
const TEXT_KEYS: &[u32] = &[
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 30, 31,
    32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 57,
];
fn keymap(text: &str) -> Result<(String, Vec<u32>)> {
    let characters: Vec<_> = text.chars().collect();
    if characters.len() > TEXT_KEYS.len() || characters.iter().any(|c| c.is_control()) {
        return Err(err("Invalid literal keyboard text"));
    }
    let mut codes = String::new();
    let mut symbols = String::new();
    for (index, character) in characters.iter().enumerate() {
        codes.push_str(&format!("<K{index:03}> = {};", TEXT_KEYS[index] + 8));
        symbols.push_str(&format!(
            "key <K{index:03}> {{ type=\"ONE_LEVEL\", [ U{:08X} ] }};",
            *character as u32
        ));
    }
    let map = format!(
        "xkb_keymap {{ xkb_keycodes \"hype\" {{ minimum=8; maximum=255; {codes} }}; xkb_types \"hype\" {{ type \"ONE_LEVEL\" {{ modifiers=None; map[None]=Level1; level_name[Level1]=\"Any\"; }}; }}; xkb_compatibility \"hype\" {{ }}; xkb_symbols \"hype\" {{ {symbols} }}; }};\0"
    );
    Ok((map, TEXT_KEYS[..characters.len()].to_vec()))
}
#[derive(Default)]
struct Keyboard {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Keyboard {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()))
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.manager = Some(registry.bind(name, 1, qh, ()))
                }
                _ => (),
            }
        }
    }
}
delegate_noop!(Keyboard: ignore wl_seat::WlSeat);
delegate_noop!(Keyboard: ignore zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1);
delegate_noop!(Keyboard: ignore zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1);

pub(crate) fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if text.chars().any(|c| c.is_control()) || text.len() > 4096 {
        return Err(err("Invalid literal keyboard text"));
    }
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = Keyboard::default();
    queue.roundtrip(&mut state)?;
    let manager = state
        .manager
        .as_ref()
        .ok_or_else(|| err("Compositor has no virtual keyboard protocol"))?;
    let seat = state
        .seat
        .as_ref()
        .ok_or_else(|| err("Compositor has no keyboard seat"))?;
    let keyboard = manager.create_virtual_keyboard(seat, &handle, ());
    let characters: Vec<_> = text.chars().collect();
    for chunk in characters.chunks(TEXT_KEYS.len()) {
        let (map, codes) = keymap(&chunk.iter().collect::<String>())?;
        let mut file = tempfile::tempfile()?;
        file.write_all(map.as_bytes())?;
        keyboard.keymap(1, file.as_fd(), map.len() as u32);
        keyboard.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut state)?;
        for code in codes {
            keyboard.key(0, code, 1);
            queue.roundtrip(&mut state)?;
            keyboard.key(0, code, 0);
            queue.roundtrip(&mut state)?;
        }
    }
    keyboard.destroy();
    queue.roundtrip(&mut state)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_sequences_use_printable_physical_keys_only() {
        let text = "👩🏽‍💻❤️🇮🇳";
        let (map, codes) = keymap(text).unwrap();
        assert_eq!(codes.len(), text.chars().count());
        assert!(
            codes
                .iter()
                .all(|code| ![1, 14, 15, 28, 29, 42, 54, 56, 58, 125].contains(code))
        );
        assert!(map.contains("U0001F469"));
        assert!(map.contains("U0000200D"));
        assert!(map.contains("U0000FE0F"));
        assert!(keymap("\n").is_err());
        assert!(keymap(&"a".repeat(49)).is_err());
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use gtk::prelude::*;
    use std::{
        cell::RefCell,
        rc::Rc,
        time::{Duration, Instant},
    };
    #[test]
    #[ignore = "requires HYPE_NATIVE_PROTOCOL_TESTS=1 and an isolated Wayland compositor"]
    fn gtk_entry_receives_complete_unicode_sequence() {
        assert_eq!(
            std::env::var("HYPE_NATIVE_PROTOCOL_TESTS").as_deref(),
            Ok("1")
        );
        gtk::init().unwrap();
        let entry = gtk::Entry::new();
        let window = gtk::Window::builder()
            .title("Hype isolated virtual keyboard fixture")
            .child(&entry)
            .build();
        let event_loop = gtk::glib::MainLoop::new(None, false);
        let outcome = Rc::new(RefCell::new(None));
        let result = outcome.clone();
        let fixture = window.clone();
        let field = entry.clone();
        let finished = event_loop.clone();
        let began = Instant::now();
        gtk::glib::timeout_add_local(Duration::from_millis(20), move || {
            if !fixture.is_active() {
                if began.elapsed() < Duration::from_secs(3) {
                    return gtk::glib::ControlFlow::Continue;
                }
                *result.borrow_mut() = Some(Err("Fixture did not receive focus".to_owned()));
                finished.quit();
                return gtk::glib::ControlFlow::Break;
            }
            field.grab_focus();
            if let Err(error) = type_text("👩🏽‍💻❤️🇮🇳") {
                *result.borrow_mut() = Some(Err(error.to_string()));
                finished.quit();
            } else {
                let field = field.clone();
                let result = result.clone();
                let finished = finished.clone();
                gtk::glib::timeout_add_local_once(Duration::from_millis(100), move || {
                    *result.borrow_mut() = Some(Ok(field.text().to_string()));
                    finished.quit();
                });
            }
            gtk::glib::ControlFlow::Break
        });
        window.present();
        entry.grab_focus();
        event_loop.run();
        window.destroy();
        assert_eq!(outcome.borrow_mut().take().unwrap().unwrap(), "👩🏽‍💻❤️🇮🇳");
    }
}
