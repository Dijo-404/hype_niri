use super::*;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

fn settle() {
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline {
        while glib::MainContext::default().iteration(false) {}
        std::thread::sleep(Duration::from_millis(3));
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    for _ in 0..20 {
        if condition() {
            return;
        }
        settle();
    }
    assert!(condition(), "service did not settle");
}
fn metadata(next: bool) -> glib::Variant {
    HashMap::from([
        (
            "xesam:title",
            if next {
                "Next test track"
            } else {
                "<b>Test & track</b>"
            }
            .to_variant(),
        ),
        ("xesam:artist", vec!["Test artist"].to_variant()),
    ])
    .to_variant()
}

#[test]
#[ignore = "requires an isolated session bus and Wayland layer-shell compositor"]
fn gtk4_parity_and_native_dbus() {
    gtk::init().expect("GTK4 display");
    assert!(gtk4_layer_shell::is_supported());
    crate::load_style().unwrap();
    let app = gtk::Application::new(
        Some("dev.hype_niri.Notch.Tests"),
        gio::ApplicationFlags::NON_UNIQUE,
    );
    app.register(None::<&gio::Cancellable>).unwrap();
    let shared = Controller::new(&app);
    assert!(
        !shared.views.borrow().is_empty(),
        "UI tests require an enabled output"
    );
    let view = shared.views.borrow()[0].clone();
    settle();
    view.collapse();
    settle();
    assert_eq!(view.window.width(), 400);
    assert_eq!(view.window.height(), 820);
    assert_eq!(view.morph.width(), 76);
    assert_eq!(view.morph.height(), 8);
    assert!(!view.body.is_child_visible());
    assert_eq!(view.window.exclusive_zone(), -1);
    view.hover();
    settle();
    assert_eq!(view.state.get(), State::Peek);
    assert_eq!(view.morph.width(), 400);
    assert!(!view.shelf.is_visible());
    assert!(!view.body.is_child_visible());
    assert!(view.morph.height() < 160);
    assert_eq!(view.window.keyboard_mode(), KeyboardMode::Exclusive);
    view.hover();
    settle();
    assert_eq!(view.state.get(), State::Open);
    assert!(!view.shelf.is_visible());
    assert!(view.body.is_child_visible());
    assert!(view.morph.height() > 200);
    let bounds = view.morph.compute_bounds(&view.window).unwrap();
    let input = view.input_region();
    assert!(!input.contains_point(bounds.x().round() as i32, bounds.y().round() as i32));
    assert!(input.contains_point(
        bounds.x().round() as i32 + 22,
        bounds.y().round() as i32 + 1
    ));

    assert_eq!(view.window.height(), 820);
    let fixture = tempfile::tempdir().unwrap();
    let files = (1..=5)
        .map(|index| {
            let path = fixture.path().join(format!("shot {index}.txt"));
            std::fs::write(&path, "shelf item").unwrap();
            CachedFile::copy(&fixture.path().join("cache"), path).unwrap()
        })
        .collect();
    shared.store_files(files);
    settle();
    assert_eq!(view.state.get(), State::Open);
    for other in shared.views.borrow().iter() {
        assert_eq!(
            other.shelf_all.tooltip_text().as_deref(),
            Some("Drag all 5 files out of the shelf")
        );
        assert!(other.shelf_clear.property::<bool>("visible"));
        assert_eq!(other.shelf_items.observe_children().n_items(), 5);
    }
    let cached = shared.shelf_files()[0].path().to_path_buf();
    assert!(view.shelf_open.is_visible());
    view.shelf_open.emit_clicked();
    settle();
    assert_eq!(view.state.get(), State::Shelf);
    let tile = view.shelf_items.first_child().unwrap();
    let remove = tile
        .last_child()
        .unwrap()
        .downcast::<gtk::Button>()
        .unwrap();
    remove.emit_clicked();
    drop(remove);
    drop(tile);
    settle();
    assert_eq!(shared.shelf_files().len(), 4);
    assert!(!cached.exists());
    assert!(fixture.path().join("shot 1.txt").exists());
    view.shelf_controls.emit_clicked();
    settle();
    assert_eq!(view.state.get(), State::Open);
    assert!(view.pinned.get());
    assert!(
        gtk::prelude::RootExt::focus(&view.window)
            .is_some_and(|focus| focus == view.search || focus.is_ancestor(&view.search))
    );
    if let Ok(path) = std::env::var("HYPE_NOTCH_SCREENSHOT") {
        let snapshot = gtk::Snapshot::new();
        let paintable = gtk::WidgetPaintable::new(Some(&view.window));
        paintable.snapshot(&snapshot, 400.0, 820.0);
        let node = snapshot.to_node().unwrap();
        view.window
            .renderer()
            .unwrap()
            .render_texture(&node, None)
            .save_to_png(path)
            .unwrap();
    }
    view.search.set_text("unmatchable-app-fixture-71941");
    settle();
    assert_eq!(view.search_apps.borrow().len(), 1);
    assert_eq!(view.results.row_at_index(0).unwrap().index(), 0);
    view.leave();
    settle();
    assert_eq!(view.state.get(), State::Open);
    view.collapse();
    settle();
    assert_eq!(view.window.keyboard_mode(), KeyboardMode::OnDemand);
    view.timer_button.emit_clicked();
    settle();
    assert!(shared.timer.borrow().running());
    for other in shared.views.borrow().iter() {
        assert_eq!(other.countdown.text(), view.countdown.text());
    }
    view.timer_button.emit_clicked();
    assert!(!shared.timer.borrow().running());
    view.watch_button.emit_clicked();
    settle();
    view.watch_button.emit_clicked();
    assert_eq!(view.watch_state.text(), "Paused");
    assert!(view.compact.property::<bool>("visible"));
    *shared.timer.borrow_mut() = FocusTimer::default();
    *shared.stopwatch.borrow_mut() = Stopwatch::default();
    shared.sync_clocks();
    assert!(!view.compact.property::<bool>("visible"));
    *shared.weather.borrow_mut() =
        crate::model::Weather::parse("Smoky haze|+26°C|+28°C|77%|Chennai, Tamil Nadu, IN");
    view.refresh_weather();
    assert_eq!(view.forecast_place.text(), "Chennai");
    assert_eq!(view.weather_temp.text(), "26°C");
    assert_eq!(view.forecast_detail.text(), "Feels 28°C · 77% humidity");
    // A completed old read must not overwrite a newer drag after its write finishes.
    view.volume.sync(15.0);
    let stale_read = shared.volume_writes.borrow().revision();
    shared.volume_writes.borrow_mut().request((15.0, None));
    view.volume.scale.set_value(77.0);
    shared.volume_writes.borrow_mut().finish();
    shared.volume_writes.borrow_mut().finish();
    view.volume_read_done(stale_read, Some(40.0));
    assert_eq!(view.volume.scale.value(), 77.0);
    // Exercise the write queue without touching real backlight hardware.
    view.brightness.device.take();
    view.brightness.scale.set_value(20.0);
    view.brightness.scale.set_value(60.0);
    view.brightness.scale.set_value(85.0);
    settle();
    assert!(!shared.brightness_writes.borrow().busy());
    assert_eq!(view.brightness.value.text(), "85%");
    view.show_message("");
    let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
    let xml = r#"<node><interface name="org.mpris.MediaPlayer2.Player">
    <method name="PlayPause"/><method name="Next"/><method name="Previous"/>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="CanPlay" type="b" access="read"/><property name="CanPause" type="b" access="read"/><property name="CanControl" type="b" access="read"/>
    <property name="CanGoNext" type="b" access="read"/><property name="CanGoPrevious" type="b" access="read"/>
    </interface></node>"#;
    let info = gio::DBusNodeInfo::for_xml(xml).unwrap();
    let interface = info
        .lookup_interface("org.mpris.MediaPlayer2.Player")
        .unwrap();
    let read_playing = Rc::new(Cell::new(true));
    let read_track = Rc::new(Cell::new(false));
    let current = Rc::clone(&read_playing);
    let track = Rc::clone(&read_track);
    let registration = bus
        .register_object("/org/mpris/MediaPlayer2", &interface)
        .property(move |_, _, _, _, property| match property {
            "PlaybackStatus" => if read_playing.get() {
                "Playing"
            } else {
                "Paused"
            }
            .to_variant(),
            "Metadata" => metadata(read_track.get()),
            "CanGoPrevious" => false.to_variant(),
            _ => true.to_variant(),
        })
        .method_call(
            move |connection, _, path, interface, method, _, invocation| {
                if method == "PlayPause" {
                    current.set(!current.get());
                }
                if method == "Next" {
                    track.set(true);
                }
                let changes = HashMap::from([
                    (
                        "PlaybackStatus",
                        if current.get() { "Playing" } else { "Paused" }.to_variant(),
                    ),
                    ("Metadata", metadata(track.get())),
                ]);
                connection
                    .emit_signal(
                        None,
                        path,
                        "org.freedesktop.DBus.Properties",
                        "PropertiesChanged",
                        Some(&(interface.unwrap(), changes, Vec::<String>::new()).to_variant()),
                    )
                    .unwrap();
                invocation.return_value(None);
            },
        )
        .build()
        .unwrap();
    let owner = gio::bus_own_name_on_connection(
        &bus,
        "org.mpris.MediaPlayer2.notchtest",
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, _| {},
    );
    until(|| view.title.text() == "<b>Test & track</b>");
    assert!(!view.title.uses_markup());
    assert!(!view.previous.is_sensitive());
    assert!(view.play.is_sensitive());
    assert!(view.header_wave.property::<bool>("visible"));
    view.play.emit_clicked();
    until(|| !view.header_wave.property::<bool>("visible"));
    assert_eq!(view.play.tooltip_text().as_deref(), Some("Play"));
    view.next.emit_clicked();
    until(|| view.title.text() == "Next test track");
    gio::bus_unown_name(owner);
    until(|| !view.media_card.property::<bool>("visible"));
    assert_eq!(view.summary.text(), "No media playing");
    bus.unregister_object(registration).unwrap();
    let xml = r#"<node><interface name="org.freedesktop.Notifications">
    <method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
    <method name="CloseNotification"><arg type="u" direction="in"/></method>
    <method name="GetCapabilities"><arg type="as" direction="out"/></method>
    <method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
    </interface><interface name="org.bluez.Device1">
    <property name="Alias" type="s" access="read"/><property name="Icon" type="s" access="read"/>
    </interface></node>"#;
    let info = gio::DBusNodeInfo::for_xml(xml).unwrap();
    let notices = Rc::new(RefCell::new(Vec::<(u32, String, String, String)>::new()));
    let seen = notices.clone();
    let server = bus
        .register_object(
            "/org/freedesktop/Notifications",
            &info
                .lookup_interface("org.freedesktop.Notifications")
                .unwrap(),
        )
        .method_call(
            move |_, _, _, _, method, parameters, invocation| match method {
                "Notify" => {
                    let (_, replaces, icon, summary, body, _, hints, _) = parameters
                        .get::<(
                            String,
                            u32,
                            String,
                            String,
                            String,
                            Vec<String>,
                            HashMap<String, glib::Variant>,
                            i32,
                        )>()
                        .unwrap();
                    let icon = hints
                        .get("image-path")
                        .and_then(|value| value.get::<String>())
                        .unwrap_or(icon);
                    seen.borrow_mut().push((replaces, summary, body, icon));
                    invocation.return_value(Some(&(7_u32,).to_variant()));
                }
                "GetCapabilities" => {
                    invocation.return_value(Some(&(vec!["body"],).to_variant()));
                }
                "GetServerInformation" => {
                    invocation.return_value(Some(&("test", "test", "1", "1.2").to_variant()));
                }
                _ => invocation.return_value(None),
            },
        )
        .build()
        .unwrap();
    let device = bus
        .register_object(
            "/org/bluez/hci0/dev_AA_BB",
            &info.lookup_interface("org.bluez.Device1").unwrap(),
        )
        .property(|_, _, _, _, property| match property {
            "Alias" => "Test keyboard".to_variant(),
            _ => "input-keyboard".to_variant(),
        })
        .build()
        .unwrap();
    let names = ["org.freedesktop.Notifications", "org.bluez"].map(|name| {
        gio::bus_own_name_on_connection(
            &bus,
            name,
            gio::BusNameOwnerFlags::NONE,
            |_, _| {},
            |_, _| {},
        )
    });
    settle();
    for connected in [true, false] {
        bus.emit_signal(
            None,
            "/org/bluez/hci0/dev_AA_BB",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(
                &(
                    "org.bluez.Device1",
                    HashMap::from([("Connected", connected.to_variant())]),
                    Vec::<String>::new(),
                )
                    .to_variant(),
            ),
        )
        .unwrap();
        let count = if connected { 1 } else { 2 };
        until(|| notices.borrow().len() == count);
    }
    {
        let notices = notices.borrow();
        assert_eq!(notices[0].1, "Test keyboard");
        assert_eq!(notices[0].2, "Connected");
        assert!(notices[0].3.contains("input-keyboard"));
        assert_eq!(notices[1].0, 7);
        assert_eq!(notices[1].2, "Disconnected");
    }
    for name in names {
        gio::bus_unown_name(name);
    }
    bus.unregister_object(device).unwrap();
    bus.unregister_object(server).unwrap();
    let group = gio::SimpleActionGroup::new();
    let status = gio::SimpleAction::new_stateful(
        "status",
        None,
        &("idle", "Ready", 0.0_f64, 0.0_f64, false).to_variant(),
    );
    group.add_action(&status);
    let toggles = Rc::new(Cell::new(0));
    for name in ["toggle", "cancel", "copy-last", "show"] {
        let action = gio::SimpleAction::new(name, None);
        let count = toggles.clone();
        action.connect_activate(move |_, _| count.set(count.get() + 1));
        group.add_action(&action);
    }
    let export = bus.export_action_group("/dev/hype/Flow", &group).unwrap();
    let owner = gio::bus_own_name_on_connection(
        &bus,
        "dev.hype.Flow",
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, _| {},
    );
    until(|| {
        shared
            .flow_actions
            .borrow()
            .as_ref()
            .is_some_and(|actions| actions.has_action("status"))
    });
    view.pin(true);
    assert_eq!(view.window.keyboard_mode(), KeyboardMode::Exclusive);
    status.set_state(&("recording", "Listening", 0.7_f64, 3.0_f64, true).to_variant());
    until(|| view.flow.reveals_child());
    settle();
    assert!(view.flow_meter.is_visible());
    assert_eq!(view.flow_status.text(), "Listening");
    assert_eq!(view.window.keyboard_mode(), KeyboardMode::None);
    view.collapse();
    settle();
    assert_eq!(view.morph.height(), 0);
    view.flow_button.emit_clicked();
    until(|| toggles.get() == 1);
    status.set_state(&("done", "Copied to clipboard", 0.0_f64, 3.0_f64, true).to_variant());
    until(|| view.flow_status.text() == "Copied to clipboard");
    assert!(view.flow_toggle.is_sensitive());
    assert!(!view.flow_cancel.is_sensitive());
    assert_eq!(view.flow_label.text(), "Copied to clipboard");

    gio::bus_unown_name(owner);
    until(|| !view.flow.reveals_child());
    settle();
    assert_eq!(view.morph.width(), 76);
    assert_eq!(view.window.keyboard_mode(), KeyboardMode::OnDemand);
    bus.unexport_action_group(export);
    // Pending callbacks must not retain a removed monitor view.
    let monitor = view.monitor.clone();
    let transient = View::new(&shared, &monitor);
    let weak = Rc::downgrade(&transient);
    transient.window.present();
    transient.set_state(State::Open);
    transient.brightness.device.take();
    transient.brightness.scale.set_value(25.0);
    transient.brightness.scale.set_value(90.0);
    transient.window.destroy();
    drop(transient);
    assert!(weak.upgrade().is_none());
    settle();
    assert!(!shared.brightness_writes.borrow().busy());
    let cached: Vec<_> = shared
        .shelf_files()
        .iter()
        .map(|file| file.path().to_path_buf())
        .collect();
    assert_eq!(cached.len(), 4);
    view.shelf_clear.emit_clicked();
    settle();
    assert!(shared.shelf_files().is_empty());
    assert!(!view.shelf_clear.property::<bool>("visible"));
    assert!(cached.iter().all(|path| !path.exists()));
    assert!(fixture.path().join("shot 2.txt").exists());
    shared.views.borrow_mut().clear();
    view.window.destroy();
    drop(view);
    drop(shared);
    settle();
}
