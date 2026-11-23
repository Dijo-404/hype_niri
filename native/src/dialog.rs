use crate::{Result, err};
use gtk::{gio, glib, prelude::*};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

thread_local! {
    static APPLICATION: RefCell<Option<gtk::Application>> = const { RefCell::new(None) };
}

fn window(
    title: &str,
    width: i32,
    height: i32,
) -> Result<Option<(gtk::Application, gtk::ApplicationWindow, glib::MainLoop)>> {
    gtk::init().map_err(|e| err(format!("Cannot open desktop dialog: {e}")))?;
    let application = APPLICATION.with(|slot| -> Result<gtk::Application> {
        if let Some(application) = slot.borrow().as_ref() {
            return Ok(application.clone());
        }
        let application = gtk::Application::new(
            Some("dev.hype_niri.Control"),
            gio::ApplicationFlags::FLAGS_NONE,
        );
        application.register(None::<&gio::Cancellable>)?;
        application.connect_activate(|application| {
            for window in application.windows() {
                window.close();
            }
        });
        *slot.borrow_mut() = Some(application.clone());
        Ok(application)
    })?;
    if application.is_remote() {
        application.activate();
        if let Some(connection) = application.dbus_connection() {
            connection.flush_sync(None::<&gio::Cancellable>)?;
        }
        return Ok(None);
    }
    let window = gtk::ApplicationWindow::builder()
        .application(&application)
        .title(title)
        .default_width(width)
        .default_height(height)
        .resizable(false)
        .build();
    window.set_widget_name("hype-native-dialog");
    let style = gtk::CssProvider::new();
    style.load_from_string("#hype-native-dialog { background: rgba(0,0,0,0.88); color:white; font-family:Roboto,sans-serif; font-size:14px; border:2px solid rgba(200,200,200,0.45); border-radius:12px; } #hype-native-dialog entry { margin:8px; padding:8px; border-radius:8px; } #hype-native-dialog list { background:transparent; } #hype-native-dialog row { padding:10px; border-radius:8px; } #hype-native-dialog row:selected { background:#e0e0e0; color:#121212; } #hype-native-dialog button { margin:8px; }");
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &style,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let event_loop = glib::MainLoop::new(None, false);
    let closed = event_loop.clone();
    window.connect_close_request(move |_| {
        closed.quit();
        glib::Propagation::Proceed
    });
    let key = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);
    Ok(Some((application, window, event_loop)))
}
pub fn choose(title: &str, choices: &[String]) -> Result<Option<usize>> {
    pick(
        title,
        &choices
            .iter()
            .map(|s| (s.clone(), None))
            .collect::<Vec<_>>(),
        false,
    )
}
pub fn images(title: &str, choices: &[(String, PathBuf)]) -> Result<Option<usize>> {
    pick(
        title,
        &choices
            .iter()
            .map(|(label, path)| (label.clone(), Some(path.clone())))
            .collect::<Vec<_>>(),
        true,
    )
}
fn pick(title: &str, choices: &[(String, Option<PathBuf>)], images: bool) -> Result<Option<usize>> {
    if choices.is_empty() {
        return Ok(None);
    }
    let Some((_application, window, event_loop)) = window(
        title,
        if images { 640 } else { 400 },
        if images { 640 } else { 420 },
    )?
    else {
        return Ok(None);
    };
    let selected = Rc::new(RefCell::new(None));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let search = gtk::Entry::new();
    search.set_placeholder_text(Some(title));
    content.append(&search);
    let list = gtk::ListBox::new();
    list.set_activate_on_single_click(true);
    for (label, path) in choices {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        if let Some(path) = path.as_ref().filter(|path| path.is_file()) {
            let image = gtk::Image::from_file(path);
            image.set_pixel_size(96);
            line.append(&image);
        }
        let label = gtk::Label::new(Some(label));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);
        line.append(&label);
        list.append(&line);
    }
    list.select_row(list.row_at_index(0).as_ref());
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    content.append(&scroll);
    window.set_child(Some(&content));
    let result = selected.clone();
    let close = window.downgrade();
    list.connect_row_activated(move |_, row| {
        *result.borrow_mut() = Some(row.index() as usize);
        if let Some(window) = close.upgrade() {
            window.close();
        }
    });
    let options: Vec<_> = choices
        .iter()
        .map(|(label, _)| label.to_lowercase())
        .collect();
    let items = list.clone();
    search.connect_changed(move |entry| {
        let query = entry.text().to_lowercase();
        let mut first = None;
        for (index, label) in options.iter().enumerate() {
            if let Some(row) = items.row_at_index(index as i32) {
                let shown = label.contains(&query);
                row.set_visible(shown);
                if shown && first.is_none() {
                    first = Some(row);
                }
            }
        }
        items.select_row(first.as_ref());
    });
    let items = list.clone();
    let close = window.downgrade();
    let result = selected.clone();
    search.connect_activate(move |_| {
        if let Some(row) = items.selected_row() {
            *result.borrow_mut() = Some(row.index() as usize);
            if let Some(window) = close.upgrade() {
                window.close();
            }
        }
    });
    let keys = gtk::EventControllerKey::new();
    let items = list.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        let direction = if key == gtk::gdk::Key::Down {
            1
        } else if key == gtk::gdk::Key::Up {
            -1
        } else {
            return glib::Propagation::Proceed;
        };
        let mut index = items.selected_row().map(|row| row.index()).unwrap_or(0) + direction;
        while let Some(row) = items.row_at_index(index) {
            if row.is_visible() {
                items.select_row(Some(&row));
                break;
            }
            index += direction;
        }
        glib::Propagation::Stop
    });
    search.add_controller(keys);
    window.present();
    search.grab_focus();
    event_loop.run();
    window.destroy();
    let result = *selected.borrow();
    Ok(result)
}
pub fn prompt(title: &str, secret: bool) -> Result<Option<String>> {
    let Some((_application, window, event_loop)) = window(title, 400, 150)? else {
        return Ok(None);
    };
    let value = Rc::new(RefCell::new(None));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let label = gtk::Label::new(Some(title));
    label.set_wrap(true);
    content.append(&label);
    let entry = gtk::Entry::new();
    entry.set_visibility(!secret);
    entry.set_input_purpose(if secret {
        gtk::InputPurpose::Password
    } else {
        gtk::InputPurpose::FreeForm
    });
    content.append(&entry);
    let accept = gtk::Button::with_label("Continue");
    content.append(&accept);
    window.set_child(Some(&content));
    let result = value.clone();
    let close = window.downgrade();
    entry.connect_activate(move |entry| {
        *result.borrow_mut() = Some(entry.text().to_string());
        if let Some(window) = close.upgrade() {
            window.close();
        }
    });
    let text = entry.clone();
    accept.connect_clicked(move |_| {
        text.emit_activate();
    });
    window.present();
    entry.grab_focus();
    event_loop.run();
    window.destroy();
    let result = value.borrow_mut().take();
    Ok(result)
}
pub fn credentials(title: &str, ssid: &str, password: &str) -> Result<()> {
    let Some((_application, window, event_loop)) = window(title, 360, 450)? else {
        return Ok(());
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    for text in [format!("Network: {ssid}"), format!("Password: {password}")] {
        let label = gtk::Label::new(Some(&text));
        label.set_selectable(true);
        label.set_wrap(true);
        content.append(&label);
    }
    let data = format!(
        "WIFI:T:WPA;S:{};P:{};;",
        wifi_escape(ssid),
        wifi_escape(password)
    );
    let qr = qrcode::QrCode::new(data.as_bytes())?;
    let drawing = gtk::DrawingArea::new();
    drawing.set_content_width(288);
    drawing.set_content_height(288);
    drawing.set_halign(gtk::Align::Center);
    drawing.set_draw_func(move |_, cr, width, height| {
        let size = qr.width();
        let unit = f64::from(width.min(height)) / (size + 8) as f64;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.paint();
        cr.set_source_rgb(0.0, 0.0, 0.0);
        for y in 0..size {
            for x in 0..size {
                if qr[(x, y)] == qrcode::Color::Dark {
                    cr.rectangle(
                        (x + 4) as f64 * unit,
                        (y + 4) as f64 * unit,
                        unit.ceil(),
                        unit.ceil(),
                    );
                }
            }
        }
        let _ = cr.fill();
    });
    content.append(&drawing);
    let close = gtk::Button::with_label("Close");
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    content.append(&close);
    window.set_child(Some(&content));
    window.present();
    event_loop.run();
    window.destroy();
    Ok(())
}
fn wifi_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if matches!(c, '\\' | ';' | ',' | '"' | ':') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn widgets(root: &gtk::Widget) -> Vec<gtk::Widget> {
        let mut found = vec![root.clone()];
        let mut child = root.first_child();
        while let Some(widget) = child {
            found.extend(widgets(&widget));
            child = widget.next_sibling();
        }
        found
    }
    fn interact(action: impl FnOnce(gtk::ApplicationWindow) + 'static) {
        glib::idle_add_local_once(move || {
            let window = gtk::Window::list_toplevels()
                .into_iter()
                .find_map(|widget| widget.downcast::<gtk::ApplicationWindow>().ok())
                .expect("native dialog window");
            action(window);
        });
    }
    #[test]
    #[ignore = "requires an isolated session bus and Wayland compositor"]
    fn gtk4_search_password_qr_and_remote_toggle() {
        interact(|window| {
            let children = widgets(window.upcast_ref());
            let search = children
                .iter()
                .find_map(|widget| widget.clone().downcast::<gtk::Entry>().ok())
                .unwrap();
            let list = children
                .iter()
                .find_map(|widget| widget.clone().downcast::<gtk::ListBox>().ok())
                .unwrap();
            search.set_text("second");
            assert!(!list.row_at_index(0).unwrap().is_visible());
            assert_eq!(list.selected_row().unwrap().index(), 1);
            search.emit_activate();
        });
        assert_eq!(
            choose("Fixture", &["First".into(), "Second".into()]).unwrap(),
            Some(1)
        );
        interact(|window| {
            let entry = widgets(window.upcast_ref())
                .into_iter()
                .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
                .unwrap();
            assert!(!gtk::prelude::EntryExt::is_visible(&entry));
            entry.set_text("private fixture password");
            entry.emit_activate();
        });
        assert_eq!(
            prompt("Unlock fixture", true).unwrap().as_deref(),
            Some("private fixture password")
        );
        interact(|window| {
            assert!(
                widgets(window.upcast_ref())
                    .iter()
                    .any(|widget| widget.is::<gtk::DrawingArea>())
            );
            window.close();
        });
        credentials(
            "Fixture credentials",
            "Test;SSID",
            "private fixture password",
        )
        .unwrap();
        interact(|window| {
            // A second process uses this same bus activation to toggle the open menu.
            window.application().unwrap().activate();
        });
        assert_eq!(
            choose("Toggle fixture", &["Never select".into()]).unwrap(),
            None
        );
    }
    #[test]
    fn qr_escapes_wifi_delimiters() {
        assert_eq!(wifi_escape("a;b:c\\d"), "a\\;b\\:c\\\\d");
    }
}
