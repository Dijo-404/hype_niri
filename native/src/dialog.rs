use crate::{Result, err};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

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
    style.load_from_string("#hype-native-dialog { background: rgba(0,0,0,0.88); color:white; font-family:Roboto,sans-serif; font-size:14px; border:2px solid rgba(200,200,200,0.45); border-radius:12px; } #hype-native-dialog entry { margin:8px; padding:8px; border-radius:8px; } #hype-native-dialog list, #hype-native-dialog listview { background:transparent; } #hype-native-dialog row { padding:10px; border-radius:8px; } #hype-native-dialog row:selected { background:#e0e0e0; color:#121212; } #hype-native-dialog button { margin:8px; }");
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
    pick(title, choices.iter().map(|label| (label, None)), None)
}

type Thumbnail = dyn Fn(&Path) -> Option<PathBuf> + Send;

pub fn images_lazy(
    title: &str,
    choices: &[(String, PathBuf)],
    thumbnail: impl Fn(&Path) -> Option<PathBuf> + Send + 'static,
) -> Result<Option<usize>> {
    pick(
        title,
        choices.iter().map(|(label, path)| (label, Some(path))),
        Some(Box::new(thumbnail)),
    )
}

struct Choice {
    index: usize,
    label: String,
    searchable: String,
    path: Option<PathBuf>,
}

fn choice(item: &glib::Object) -> &glib::BoxedAnyObject {
    item.downcast_ref().expect("Picker model contains choices")
}

fn pick<'a>(
    title: &str,
    choices: impl ExactSizeIterator<Item = (&'a String, Option<&'a PathBuf>)>,
    thumbnail: Option<Box<Thumbnail>>,
) -> Result<Option<usize>> {
    if choices.len() == 0 {
        return Ok(None);
    }
    let images = thumbnail.is_some();
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

    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let choices: Vec<_> = choices
        .enumerate()
        .map(|(index, (label, path))| {
            glib::BoxedAnyObject::new(Choice {
                index,
                label: label.clone(),
                searchable: label.to_lowercase(),
                path: path.cloned(),
            })
        })
        .collect();
    model.extend_from_slice(&choices);
    let query = Rc::new(RefCell::new(String::new()));
    let filter = gtk::CustomFilter::new({
        let query = query.clone();
        move |item| {
            choice(item)
                .borrow::<Choice>()
                .searchable
                .contains(query.borrow().as_str())
        }
    });
    let filtered = gtk::FilterListModel::new(Some(model), Some(filter.clone()));
    let selection = gtk::SingleSelection::new(Some(filtered));
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        if images {
            let image = gtk::Image::new();
            image.set_pixel_size(96);
            image.set_size_request(96, 96);
            row.append(&image);
        }
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);
        row.append(&label);
        item.set_child(Some(&row));
    });
    let thumbnail = thumbnail.map(|load| Arc::new(Mutex::new(load)));
    let pending = Rc::new(RefCell::new(
        HashMap::<gtk::ListItem, gio::Cancellable>::new(),
    ));
    factory.connect_bind({
        let pending = pending.clone();
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let value = item.item().unwrap();
            let value = choice(&value).borrow::<Choice>();
            let row = item.child().unwrap();
            let label = row.last_child().unwrap().downcast::<gtk::Label>().unwrap();
            label.set_label(&value.label);
            if let (Some(load), Some(path)) = (thumbnail.clone(), value.path.clone()) {
                let image = row.first_child().unwrap().downcast::<gtk::Image>().unwrap();
                image.clear();
                let image = image.downgrade();
                let cancel = gio::Cancellable::new();
                pending.borrow_mut().insert(item.clone(), cancel.clone());
                glib::spawn_future_local(async move {
                    let cancelled = cancel.clone();
                    let result = gio::spawn_blocking(move || {
                        // Cap concurrent decoding of full-sized source images.
                        let load = load.lock().ok()?;
                        if cancelled.is_cancelled() {
                            None
                        } else {
                            load(&path)
                        }
                    })
                    .await;
                    if !cancel.is_cancelled()
                        && let (Some(image), Ok(Some(path))) = (image.upgrade(), result)
                    {
                        image.set_from_file(Some(path));
                    }
                });
            }
        }
    });
    factory.connect_unbind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(cancel) = pending.borrow_mut().remove(item) {
            cancel.cancel();
        }
    });
    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.set_single_click_activate(true);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    content.append(&scroll);
    window.set_child(Some(&content));

    let activate = {
        let result = selected.clone();
        let close = window.downgrade();
        move |selection: &gtk::SingleSelection, position| {
            if let Some(item) = selection.item(position) {
                *result.borrow_mut() = Some(choice(&item).borrow::<Choice>().index);
                if let Some(window) = close.upgrade() {
                    window.close();
                }
            }
        }
    };
    let items = selection.clone();
    let activate_row = activate.clone();
    list.connect_activate(move |_, position| activate_row(&items, position));
    let items = selection.clone();
    search.connect_activate(move |_| activate(&items, items.selected()));
    let items = selection.clone();
    search.connect_changed(move |entry| {
        *query.borrow_mut() = entry.text().to_lowercase();
        filter.changed(gtk::FilterChange::Different);
        items.set_selected(0);
    });
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, _| {
        let current = selection.selected();
        let next = match key {
            gtk::gdk::Key::Down => current.saturating_add(1),
            gtk::gdk::Key::Up => current.saturating_sub(1),
            _ => return glib::Propagation::Proceed,
        };
        if next < selection.n_items() {
            selection.set_selected(next);
            list.scroll_to(next, gtk::ListScrollFlags::NONE, None);
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
                .find_map(|widget| widget.clone().downcast::<gtk::ListView>().ok())
                .unwrap();
            search.set_text("second");
            let selection = list
                .model()
                .unwrap()
                .downcast::<gtk::SingleSelection>()
                .unwrap();
            assert_eq!(selection.n_items(), 1);
            let item = selection.selected_item().unwrap();
            assert_eq!(choice(&item).borrow::<Choice>().index, 1);
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
