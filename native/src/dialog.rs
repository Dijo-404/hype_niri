use crate::{Result, err};
use futures_channel::oneshot;
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

const STYLE: &str = r#"
    #hype-native-dialog {
        background: rgba(20, 20, 20, 0.87);
        color: #d4d4d4;
        border: 2px solid #fff;
        border-radius: 4px;
        font-family: "JetBrains Mono", "Symbols Nerd Font", monospace;
        font-size: 11pt;
        box-shadow: none;
    }
    #hype-native-dialog entry {
        margin: 8px;
        padding: 4px 6px;
        border-radius: 4px;
    }
    #hype-native-dialog .picker-search-row {
        margin: 15px 20px 8px;
        padding: 0 6px;
    }
    #hype-native-dialog .picker-search {
        margin: 0;
        min-height: 22px;
        padding: 0;
        background: transparent;
        color: #d4d4d4;
        border: none;
        box-shadow: none;
        outline: none;
    }
    #hype-native-dialog .picker-search placeholder { color: #aaa; opacity: 1; }
    #hype-native-dialog scrolledwindow { margin: 0 20px 19px; }
    #hype-native-dialog listview { background: transparent; color: #d4d4d4; }
    #hype-native-dialog row {
        min-height: 22px;
        padding: 0 6px;
        border-radius: 4px;
        transition: background-color 180ms ease-out, color 180ms ease-out;
    }
    #hype-native-dialog row:hover { background: #303030; color: #fff; }
    #hype-native-dialog row:selected { background: #404040; color: #fff; }
    #hype-native-dialog button { margin: 8px; }
"#;

fn window(title: &str, width: i32, height: i32) -> Result<Option<gtk::ApplicationWindow>> {
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
    style.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &style,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
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
    Ok(Some(window))
}

async fn show(window: &gtk::ApplicationWindow, focus: &impl IsA<gtk::Widget>) {
    let (sender, closed) = oneshot::channel();
    let sender = RefCell::new(Some(sender));
    window.connect_close_request(move |_| {
        if let Some(sender) = sender.take() {
            let _ = sender.send(());
        }
        glib::Propagation::Proceed
    });
    // Keep a single GLib event loop so thumbnail tasks can run while the picker is open.
    window.present();
    focus.grab_focus();
    let _ = closed.await;
    window.destroy();
}

pub async fn choose(title: &str, choices: &[String]) -> Result<Option<usize>> {
    pick(
        title,
        choices
            .iter()
            .enumerate()
            .map(|(index, label)| Choice::new(index, label.clone(), None)),
        PickerOptions::default(),
    )
    .await
}

pub async fn choose_with_keys(
    title: &str,
    choices: &[String],
    keys: &[char],
) -> Result<Option<usize>> {
    if choices.len() != keys.len() {
        return Err(err("Every picker row needs a shortcut key"));
    }
    let rows = choices
        .iter()
        .zip(keys)
        .enumerate()
        .map(|(index, (label, key))| {
            let mut row = Choice::new(index, label.clone(), None);
            row.label = format!("{label}  [{}]", key.to_ascii_uppercase());
            row
        });
    pick(
        title,
        rows,
        PickerOptions {
            keys: keys.to_vec(),
            ..Default::default()
        },
    )
    .await
}

pub async fn choose_apps(title: &str, apps: &[gio::AppInfo]) -> Result<Option<usize>> {
    let fallback = gio::ThemedIcon::new("application-x-executable");
    let choices = apps.iter().enumerate().map(|(index, app)| {
        let icon = app.icon().unwrap_or_else(|| fallback.clone().upcast());
        Choice::app(index, app, icon)
    });
    pick(
        title,
        choices,
        PickerOptions {
            image_size: Some(20),
            rank_apps: true,
            ..Default::default()
        },
    )
    .await
}

type Thumbnail = dyn Fn(&Path) -> Option<PathBuf> + Send;

#[derive(Default)]
struct PickerOptions {
    image_size: Option<i32>,
    thumbnail: Option<Box<Thumbnail>>,
    rank_apps: bool,
    keys: Vec<char>,
}

enum Artwork {
    Icon(gio::Icon),
    Preview(PathBuf),
}

pub async fn images_lazy(
    title: &str,
    choices: &[(String, PathBuf)],
    thumbnail: impl Fn(&Path) -> Option<PathBuf> + Send + 'static,
) -> Result<Option<usize>> {
    pick(
        title,
        choices
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, (label, path))| Choice::new(index, label, Some(Artwork::Preview(path)))),
        PickerOptions {
            image_size: Some(96),
            thumbnail: Some(Box::new(thumbnail)),
            ..Default::default()
        },
    )
    .await
}

struct Choice {
    index: usize,
    label: String,
    searchable: Vec<String>,
    artwork: Option<Artwork>,
}

impl Choice {
    fn new(index: usize, label: String, artwork: Option<Artwork>) -> Self {
        Self {
            index,
            searchable: vec![label.to_lowercase()],
            label,
            artwork,
        }
    }

    fn app(index: usize, app: &gio::AppInfo, icon: gio::Icon) -> Self {
        let mut choice = Self::new(
            index,
            app.display_name().to_string(),
            Some(Artwork::Icon(icon)),
        );
        choice.searchable.push(app.name().to_lowercase());
        choice.searchable.extend(
            app.downcast_ref::<gio_unix::DesktopAppInfo>()
                .and_then(|entry| entry.generic_name())
                .map(|generic| generic.to_lowercase()),
        );
        choice
            .searchable
            .extend(app.id().map(|id| id.to_lowercase()));
        choice
    }
}

// Exact, prefix, and contiguous matches outrank gapped matches. Prefer a closer
// match within one field, with the original list order as the final tiebreaker.
fn app_match(fields: &[String], query: &str) -> Option<(u8, usize, usize, usize)> {
    fields
        .iter()
        .enumerate()
        .filter_map(|(field, value)| {
            if value == query {
                return Some((0, 0, field, 0));
            }
            if let Some(start) = value.find(query) {
                return Some((if start == 0 { 1 } else { 2 }, 0, field, start));
            }
            let mut remaining = query.chars();
            let mut next = remaining.next()?;
            let mut start = None;
            for (index, character) in value.char_indices() {
                if character != next {
                    continue;
                }
                let first = *start.get_or_insert(index);
                let Some(key) = remaining.next() else {
                    let gap = index + character.len_utf8() - first - query.len();
                    return Some((3, gap, field, first));
                };
                next = key;
            }
            None
        })
        .min()
}

fn choice(item: &glib::Object) -> &glib::BoxedAnyObject {
    item.downcast_ref().expect("Picker model contains choices")
}

async fn pick(
    title: &str,
    choices: impl ExactSizeIterator<Item = Choice>,
    options: PickerOptions,
) -> Result<Option<usize>> {
    if choices.len() == 0 {
        return Ok(None);
    }
    let PickerOptions {
        image_size,
        thumbnail,
        rank_apps,
        keys: shortcuts,
    } = options;
    let previews = thumbnail.is_some();
    let Some(window) = window(
        title,
        if previews { 540 } else { 394 },
        if previews { 480 } else { 328 },
    )?
    else {
        return Ok(None);
    };
    window.set_decorated(false);
    let selected = Rc::new(RefCell::new(None));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    search_row.add_css_class("picker-search-row");
    search_row.append(&gtk::Label::new(Some(">")));
    let search = gtk::Entry::new();
    search.add_css_class("picker-search");
    search.set_hexpand(true);
    search.set_placeholder_text(Some(title));
    search_row.append(&search);
    content.append(&search_row);

    let model = gio::ListStore::new::<glib::BoxedAnyObject>();
    let choices: Vec<_> = choices.map(glib::BoxedAnyObject::new).collect();
    model.extend_from_slice(&choices);
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, if previews { 12 } else { 8 });
        if let Some(size) = image_size {
            let image = gtk::Image::new();
            image.set_pixel_size(size);
            image.set_size_request(size, size);
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
            let Some(artwork) = &value.artwork else {
                return;
            };
            let image = row.first_child().unwrap().downcast::<gtk::Image>().unwrap();
            match (artwork, &thumbnail) {
                (Artwork::Icon(icon), _) => image.set_from_gicon(icon),
                (Artwork::Preview(path), Some(load)) => {
                    let load = load.clone();
                    let path = path.clone();
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
                _ => {}
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
        .vscrollbar_policy(gtk::PolicyType::External)
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
    let activate_entry = activate.clone();
    search.connect_activate(move |_| activate_entry(&items, items.selected()));
    let items = selection.clone();
    search.connect_changed(move |entry| {
        let query = entry.text().to_lowercase();
        let query = if rank_apps {
            query.trim()
        } else {
            query.as_str()
        };
        if query.is_empty() {
            model.splice(0, model.n_items(), &choices);
        } else {
            let mut matches: Vec<_> = choices
                .iter()
                .filter_map(|item| {
                    let row = item.borrow::<Choice>();
                    let rank = if rank_apps {
                        app_match(&row.searchable, query)
                    } else {
                        row.searchable[0].contains(query).then_some((0, 0, 0, 0))
                    };
                    rank.map(|rank| (rank, row.index, item))
                })
                .collect();
            matches.sort_unstable_by_key(|&(rank, index, _)| (rank, index));
            let rows: Vec<_> = matches
                .into_iter()
                .map(|(_, _, item)| item.clone())
                .collect();
            model.splice(0, model.n_items(), &rows);
        }
        items.set_selected(0);
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let shortcut_result = selected.clone();
    let close = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, state| {
        use gtk::gdk::{Key, ModifierType as Mod};
        let modifiers = state & gtk::accelerator_get_default_mod_mask();
        if modifiers.difference(Mod::SHIFT_MASK).is_empty()
            && let Some(index) = key.to_unicode().and_then(|key| {
                shortcuts
                    .iter()
                    .position(|shortcut| shortcut.eq_ignore_ascii_case(&key))
            })
        {
            *shortcut_result.borrow_mut() = Some(index);
            if let Some(window) = close.upgrade() {
                window.close();
            }
            return glib::Propagation::Stop;
        }
        let control = modifiers == Mod::CONTROL_MASK;
        let current = selection.selected();
        let count = selection.n_items();
        let page = || {
            let row = list.first_child().map_or(1, |row| row.height()).max(1);
            (scroll.height() / row).max(1) as u32
        };
        let next = match key {
            Key::Down | Key::KP_Down => current.saturating_add(1),
            Key::Up | Key::KP_Up => current.saturating_sub(1),
            Key::n if control => current.saturating_add(1),
            Key::p if control => current.saturating_sub(1),
            Key::Home | Key::KP_Home if control => 0,
            Key::End | Key::KP_End if control => count.saturating_sub(1),
            Key::Page_Up | Key::KP_Page_Up => current.saturating_sub(page()),
            Key::Page_Down | Key::KP_Page_Down => {
                current.saturating_add(page()).min(count.saturating_sub(1))
            }
            Key::Tab if rank_apps && modifiers.is_empty() => {
                if count == 1 {
                    activate(&selection, current);
                    return glib::Propagation::Stop;
                }
                let next = current.saturating_add(1);
                if next < count { next } else { 0 }
            }
            Key::ISO_Left_Tab if rank_apps && modifiers == Mod::SHIFT_MASK => {
                current.checked_sub(1).unwrap_or(count.saturating_sub(1))
            }
            Key::y if control => {
                activate(&selection, current);
                return glib::Propagation::Stop;
            }
            Key::g if control => {
                if let Some(window) = close.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            _ => return glib::Propagation::Proceed,
        };
        if next < count {
            selection.set_selected(next);
            list.scroll_to(next, gtk::ListScrollFlags::NONE, None);
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    show(&window, &search).await;
    let result = *selected.borrow();
    Ok(result)
}
pub async fn prompt(title: &str, secret: bool) -> Result<Option<String>> {
    let Some(window) = window(title, 400, 150)? else {
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
    show(&window, &entry).await;
    let result = value.borrow_mut().take();
    Ok(result)
}
pub async fn credentials(title: &str, ssid: &str, password: &str) -> Result<()> {
    let Some(window) = window(title, 360, 450)? else {
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
    show(&window, &close).await;
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

    fn press_picker_key(
        window: &gtk::ApplicationWindow,
        key: gtk::gdk::Key,
        modifiers: gtk::gdk::ModifierType,
    ) {
        let controllers = window.observe_controllers();
        let keys = (0..controllers.n_items())
            .filter_map(|index| {
                controllers
                    .item(index)?
                    .downcast::<gtk::EventControllerKey>()
                    .ok()
            })
            .find(|controller| controller.propagation_phase() == gtk::PropagationPhase::Capture)
            .expect("picker key controller");
        assert!(keys.emit_by_name::<bool>("key-pressed", &[&key, &0_u32, &modifiers]));
    }
    #[test]
    #[ignore = "requires an isolated session bus and Wayland compositor"]
    fn gtk4_search_password_qr_and_remote_toggle() {
        glib::MainContext::default().block_on(search_password_qr_and_remote_toggle());
    }

    async fn search_password_qr_and_remote_toggle() {
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
            choose("Fixture", &["First".into(), "Second".into()])
                .await
                .unwrap(),
            Some(1)
        );
        interact(|window| {
            use gtk::gdk::{Key, ModifierType};
            for key in [Key::n, Key::p, Key::n, Key::y] {
                press_picker_key(&window, key, ModifierType::CONTROL_MASK);
            }
        });
        assert_eq!(
            choose("Navigation fixture", &["First".into(), "Second".into()])
                .await
                .unwrap(),
            Some(1)
        );
        interact(|window| {
            press_picker_key(&window, gtk::gdk::Key::r, gtk::gdk::ModifierType::empty());
        });
        assert_eq!(
            choose_with_keys("Power", &["Lock".into(), "Reboot".into()], &['l', 'r'])
                .await
                .unwrap(),
            Some(1)
        );
        let apps = [
            ("Terminal fixture", Some("utilities-terminal")),
            ("Fallback fixture", None),
        ]
        .map(|(name, icon)| {
            let entry = glib::KeyFile::new();
            entry.set_string("Desktop Entry", "Type", "Application");
            entry.set_string("Desktop Entry", "Name", name);
            entry.set_string("Desktop Entry", "Exec", "/usr/bin/true");
            if let Some(icon) = icon {
                entry.set_string("Desktop Entry", "Icon", icon);
            } else {
                entry.set_string("Desktop Entry", "GenericName", "System Monitor");
            }
            gio_unix::DesktopAppInfo::from_keyfile(&entry)
                .unwrap()
                .upcast()
        });
        interact(|window| {
            let children = widgets(window.upcast_ref());
            let icons: Vec<_> = children
                .iter()
                .filter_map(|widget| widget.downcast_ref::<gtk::Image>())
                .filter(|image| image.pixel_size() == 20)
                .filter_map(gtk::Image::gicon)
                .collect();
            for expected in ["utilities-terminal", "application-x-executable"] {
                let expected = gio::ThemedIcon::new(expected);
                assert!(icons.iter().any(|icon| icon.equal(Some(&expected))));
            }
            let search = children
                .iter()
                .find_map(|widget| widget.downcast_ref::<gtk::Entry>())
                .unwrap();
            let prefix = search.prev_sibling().and_downcast::<gtk::Label>().unwrap();
            assert_eq!(prefix.label(), ">");
            search.set_text("sysmon");
            press_picker_key(&window, gtk::gdk::Key::Tab, gtk::gdk::ModifierType::empty());
        });
        assert_eq!(choose_apps("Applications", &apps).await.unwrap(), Some(1));
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
            prompt("Unlock fixture", true).await.unwrap().as_deref(),
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
        .await
        .unwrap();
        interact(|window| {
            // A second process uses this same bus activation to toggle the open menu.
            window.application().unwrap().activate();
        });
        assert_eq!(
            choose("Toggle fixture", &["Never select".into()])
                .await
                .unwrap(),
            None
        );
    }

    #[test]
    #[ignore = "requires an isolated session bus and Wayland compositor"]
    fn gtk4_wallpaper_preview_loads_inside_the_control_event_loop() {
        let fixture = tempfile::tempdir().unwrap();
        let image = fixture.path().join("preview.png");
        image::RgbImage::new(2, 2).save(&image).unwrap();
        interact(|window| {
            let children = widgets(window.upcast_ref());
            let search = children
                .iter()
                .find_map(|widget| widget.clone().downcast::<gtk::Entry>().ok())
                .unwrap();
            let preview = children
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::Image>().ok())
                .find(|image| image.pixel_size() == 96)
                .unwrap();
            if preview.paintable().is_some() {
                search.emit_activate();
            } else {
                preview.connect_paintable_notify(move |_| search.emit_activate());
            }
            glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || window.close());
        });
        let choices = [("Wallpaper fixture".into(), image)];
        let result = glib::MainContext::default()
            .block_on(images_lazy("Wallpaper", &choices, |image| {
                Some(image.to_owned())
            }))
            .unwrap();
        assert_eq!(result, Some(0));
    }
    #[test]
    fn app_search_ranks_names_and_matches_desktop_metadata() {
        let fields = |label: &str| vec![label.to_owned()];
        let exact = app_match(&fields("fire"), "fire").unwrap();
        let prefix = app_match(&fields("firefox"), "fire").unwrap();
        let substring = app_match(&fields("waterfire"), "fire").unwrap();
        let fuzzy = app_match(&fields("firefox"), "ffx").unwrap();
        assert!(exact < prefix && prefix < substring && substring < fuzzy);
        assert!(app_match(&fields("firefox"), "rff").is_none());
        let app = vec![
            "files".into(),
            "file manager".into(),
            "org.gnome.nautilus.desktop".into(),
        ];
        assert!(app_match(&app, "nautilus").is_some());
        assert!(app_match(&app, "file man").is_some());
        assert!(app_match(&app, "wallpaper").is_none());
    }

    #[test]
    fn qr_escapes_wifi_delimiters() {
        assert_eq!(wifi_escape("a;b:c\\d"), "a\\;b\\:c\\\\d");
    }
}
