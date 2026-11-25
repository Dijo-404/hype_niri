use crate::{
    model::{FocusTimer, Stopwatch, Weather, WriteQueue},
    services::{Audio, Backlight, Media},
    shelf::{CachedFile, Shelf},
    ui::View,
};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct FlowState {
    pub state: String,
    pub message: String,
    pub level: f64,
    pub visible: bool,
}
impl Default for FlowState {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            message: "Open Hype Flow to start dictation".into(),
            level: 0.0,
            visible: false,
        }
    }
}
impl FlowState {
    pub fn recording(&self) -> bool {
        matches!(self.state.as_str(), "recording" | "starting")
    }
    pub fn parse(value: &glib::Variant) -> Option<Self> {
        let (state, message, level, _elapsed, visible) =
            value.get::<(String, String, f64, f64, bool)>()?;
        Some(Self {
            state,
            message,
            level: if level.is_finite() {
                level.clamp(0.0, 1.0)
            } else {
                0.0
            },
            visible,
        })
    }
}

pub type LevelRequest = (f64, Option<Backlight>);

pub struct Controller {
    pub audio: Audio,
    pub volume_writes: RefCell<WriteQueue<LevelRequest>>,
    pub brightness_writes: RefCell<WriteQueue<LevelRequest>>,
    pub app: gtk::Application,
    pub views: RefCell<Vec<Rc<View>>>,
    pub timer: RefCell<FocusTimer>,
    pub stopwatch: RefCell<Stopwatch>,
    pub weather: RefCell<Option<Weather>>,
    pub weather_loaded: Cell<bool>,
    pub flow: RefCell<FlowState>,
    pub flow_levels: RefCell<[f64; 64]>,
    pub flow_actions: RefCell<Option<gio::DBusActionGroup>>,
    pub media: RefCell<BTreeMap<String, gio::DBusProxy>>,
    pub selected: RefCell<Option<gio::DBusProxy>>,
    clock: RefCell<Option<glib::SourceId>>,
    shelf: RefCell<Shelf>,
    shelf_source: RefCell<Option<glib::SourceId>>,
    weather_source: RefCell<Option<glib::SourceId>>,
    flow_watch: RefCell<Option<gio::WatcherId>>,
    bus_signal: RefCell<Option<gio::SignalSubscription>>,
    weather_busy: Cell<bool>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}
impl Controller {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let this = Rc::new(Self {
            app: app.clone(),
            audio: Audio::default(),
            volume_writes: RefCell::new(WriteQueue::default()),
            brightness_writes: RefCell::new(WriteQueue::default()),
            views: RefCell::new(Vec::new()),
            timer: RefCell::new(FocusTimer::default()),
            stopwatch: RefCell::new(Stopwatch::default()),
            weather: RefCell::new(None),
            weather_loaded: Cell::new(false),
            flow: RefCell::new(FlowState::default()),
            flow_levels: RefCell::new([0.0; 64]),
            flow_actions: RefCell::new(None),
            media: RefCell::new(BTreeMap::new()),
            selected: RefCell::new(None),
            clock: RefCell::new(None),
            shelf: RefCell::new(Shelf::default()),
            shelf_source: RefCell::new(None),
            weather_source: RefCell::new(None),
            flow_watch: RefCell::new(None),
            bus_signal: RefCell::new(None),
            weather_busy: Cell::new(false),
            hold: RefCell::new(Some(app.hold())),
        });
        let capability = gio::SimpleAction::new_stateful("dictation", None, &true.to_variant());
        app.add_action(&capability);
        this.sync_monitors();
        let monitors = gtk::gdk::Display::default().unwrap().monitors();
        let weak = Rc::downgrade(&this);
        monitors.connect_items_changed(move |_, _, _, _| {
            if let Some(this) = weak.upgrade() {
                this.sync_monitors();
            }
        });
        this.start_services();
        this
    }
    fn sync_monitors(self: &Rc<Self>) {
        let monitors = gtk::gdk::Display::default().unwrap().monitors();
        let current: Vec<_> = (0..monitors.n_items())
            .filter_map(|i| monitors.item(i)?.downcast::<gtk::gdk::Monitor>().ok())
            .collect();
        // Stay alive with no monitors connected; views return on monitor-added.
        self.views.borrow_mut().retain(|view| {
            if current.contains(&view.monitor) {
                true
            } else {
                view.window.destroy();
                false
            }
        });
        for monitor in current {
            if self
                .views
                .borrow()
                .iter()
                .any(|view| view.monitor == monitor)
            {
                continue;
            }
            let view = View::new(self, &monitor);
            self.views.borrow_mut().push(view.clone());
            view.window.present();
            view.refresh_clocks();
            view.refresh_weather();
            view.refresh_media(&self.media_state());
            view.refresh_flow();
            view.refresh_shelf(&self.shelf_files());
        }
    }
    pub fn shelf_files(&self) -> Vec<Rc<CachedFile>> {
        self.shelf.borrow().files()
    }
    pub fn store_files(self: &Rc<Self>, files: Vec<CachedFile>) {
        if !files.is_empty() {
            self.shelf.borrow_mut().add(files, Instant::now());
            self.sync_shelf();
        }
    }
    pub fn remove_file(self: &Rc<Self>, path: &Path) {
        if self.shelf.borrow_mut().remove(path) {
            self.sync_shelf();
        }
    }
    fn sync_shelf(self: &Rc<Self>) {
        if let Some(source) = self.shelf_source.take() {
            source.remove();
        }
        let now = Instant::now();
        self.shelf.borrow_mut().expire(now);
        let files = self.shelf_files();
        for view in self.views.borrow().iter() {
            view.refresh_shelf(&files);
        }
        if let Some(expires) = self.shelf.borrow().next_expiry() {
            let weak = Rc::downgrade(self);
            let delay = expires
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1));
            *self.shelf_source.borrow_mut() =
                Some(glib::timeout_add_local_once(delay, move || {
                    if let Some(this) = weak.upgrade() {
                        this.shelf_source.take();
                        this.sync_shelf();
                    }
                }));
        }
    }
    pub fn notify(&self, completed: Option<&str>) {
        let Some(phase) = completed else {
            return;
        };
        let notification = gio::Notification::new(&format!("{phase} complete"));
        notification.set_body(Some(if phase == "Focus" {
            "Take a five-minute break."
        } else {
            "Ready for another focus session."
        }));
        self.app
            .send_notification(Some("focus-timer"), &notification);
    }
    pub fn sync_clocks(self: &Rc<Self>) {
        for view in self.views.borrow().iter() {
            view.refresh_clocks();
        }
        let running = self.timer.borrow().running() || self.stopwatch.borrow().running;
        if !running {
            if let Some(source) = self.clock.take() {
                source.remove();
            }
            return;
        }
        if self.clock.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        *self.clock.borrow_mut() = Some(glib::timeout_add_local(
            std::time::Duration::from_millis(100),
            move || {
                let Some(this) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let completed = this.timer.borrow_mut().update(glib::monotonic_time());
                this.notify(completed);
                for view in this.views.borrow().iter() {
                    view.refresh_clocks();
                }
                if this.timer.borrow().running() || this.stopwatch.borrow().running {
                    glib::ControlFlow::Continue
                } else {
                    this.clock.take();
                    glib::ControlFlow::Break
                }
            },
        ));
    }
    fn start_services(self: &Rc<Self>) {
        self.fetch_weather();
        prune_shelf();
        let shelf = Rc::downgrade(self);
        glib::timeout_add_local(crate::shelf::LIFETIME, move || {
            if shelf.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            prune_shelf();
            glib::ControlFlow::Continue
        });
        let weak = Rc::downgrade(self);
        *self.weather_source.borrow_mut() =
            Some(glib::timeout_add_seconds_local(1800, move || {
                if let Some(this) = weak.upgrade() {
                    this.fetch_weather();
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            }));
        let appeared = Rc::downgrade(self);
        let vanished = Rc::downgrade(self);
        *self.flow_watch.borrow_mut() = Some(gio::bus_watch_name(
            gio::BusType::Session,
            "dev.hype.Flow",
            gio::BusNameWatcherFlags::NONE,
            move |bus, name, _| {
                let Some(this) = appeared.upgrade() else {
                    return;
                };
                let actions = gio::DBusActionGroup::get(&bus, Some(name), "/dev/hype/Flow");
                let weak = Rc::downgrade(&this);
                actions.connect_action_state_changed(None, move |_, name, value| {
                    if name == "status"
                        && let Some(this) = weak.upgrade()
                    {
                        this.update_flow(value);
                    }
                });
                let weak = Rc::downgrade(&this);
                actions.connect_action_added(None, move |actions, name| {
                    if name == "status"
                        && let (Some(this), Some(value)) =
                            (weak.upgrade(), actions.action_state("status"))
                    {
                        this.update_flow(&value);
                    }
                });
                *this.flow_actions.borrow_mut() = Some(actions.clone());
                if let Some(value) = actions.action_state("status") {
                    this.update_flow(&value);
                }
                for view in this.views.borrow().iter() {
                    view.refresh_flow();
                }
            },
            move |_, _| {
                if let Some(this) = vanished.upgrade() {
                    this.flow_actions.take();
                    *this.flow.borrow_mut() = FlowState::default();
                    for view in this.views.borrow().iter() {
                        view.refresh_flow();
                    }
                }
            },
        ));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(bus) = gio::bus_get_future(gio::BusType::Session).await else {
                return;
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            let changed = Rc::downgrade(&this);
            let signal = bus.subscribe_to_signal(
                Some("org.freedesktop.DBus"),
                Some("org.freedesktop.DBus"),
                Some("NameOwnerChanged"),
                Some("/org/freedesktop/DBus"),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    let Some((name, _, owner)) =
                        signal.parameters.get::<(String, String, String)>()
                    else {
                        return;
                    };
                    if !name.starts_with("org.mpris.MediaPlayer2.") {
                        return;
                    }
                    if let Some(this) = changed.upgrade() {
                        this.media.borrow_mut().remove(&name);
                        this.refresh_media();
                        if !owner.is_empty() {
                            this.add_player(&name);
                        }
                    }
                },
            );
            *this.bus_signal.borrow_mut() = Some(signal);
            drop(this);
            if let Ok(reply) = bus
                .call_future(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "ListNames",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    3000,
                )
                .await
                && let (Some(this), Some((names,))) =
                    (weak.upgrade(), reply.get::<(Vec<String>,)>())
            {
                for name in names {
                    if name.starts_with("org.mpris.MediaPlayer2.") {
                        this.add_player(&name);
                    }
                }
            }
        });
    }
    fn fetch_weather(self: &Rc<Self>) {
        if self.weather_busy.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let weather = gio::spawn_blocking(crate::services::weather).await;
            if let Some(this) = weak.upgrade() {
                this.weather_busy.set(false);
                this.weather_loaded.set(true);
                *this.weather.borrow_mut() = weather.ok().and_then(|result| result.ok());
                for view in this.views.borrow().iter() {
                    view.refresh_weather();
                }
            }
        });
    }
    fn add_player(self: &Rc<Self>, name: &str) {
        let name = name.to_owned();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(proxy) = gio::DBusProxy::for_bus_future(
                gio::BusType::Session,
                gio::DBusProxyFlags::DO_NOT_AUTO_START,
                None,
                &name,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            )
            .await
            else {
                return;
            };
            if proxy.name_owner().is_none() {
                return;
            }
            let Some(this) = weak.upgrade() else {
                return;
            };
            let changed = Rc::downgrade(&this);
            proxy.connect_local("g-properties-changed", false, move |_| {
                if let Some(this) = changed.upgrade() {
                    this.refresh_media();
                }
                None
            });
            this.media.borrow_mut().insert(name, proxy);
            this.refresh_media();
        });
    }
    fn media_state(&self) -> Media {
        let players = self.media.borrow();
        let selected = players
            .values()
            .find(|proxy| Media::from_proxy(proxy).playing)
            .or_else(|| {
                players
                    .values()
                    .find(|proxy| Media::from_proxy(proxy).active())
            })
            .or_else(|| players.values().next());
        *self.selected.borrow_mut() = selected.cloned();
        selected.map(Media::from_proxy).unwrap_or_default()
    }
    fn refresh_media(&self) {
        let state = self.media_state();
        for view in self.views.borrow().iter() {
            view.refresh_media(&state);
        }
    }
    pub fn media_action(self: &Rc<Self>, method: &str) {
        let Some(proxy) = self.selected.borrow().clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        let method = method.to_owned();
        glib::spawn_future_local(async move {
            if let Err(error) = proxy
                .call_future(&method, None, gio::DBusCallFlags::NONE, 3000)
                .await
                && let Some(this) = weak.upgrade()
            {
                this.show_error(&error.to_string());
            }
        });
    }
    pub fn update_flow(&self, value: &glib::Variant) {
        let Some(state) = FlowState::parse(value) else {
            return;
        };
        let mut levels = self.flow_levels.borrow_mut();
        levels.rotate_left(1);
        levels[63] = state.level;
        drop(levels);
        *self.flow.borrow_mut() = state;
        for view in self.views.borrow().iter() {
            view.refresh_flow();
        }
    }
    pub fn flow_action(self: &Rc<Self>, action: &str) {
        if let Some(actions) = self.flow_actions.borrow().as_ref()
            && actions.has_action(action)
        {
            actions.activate_action(action, None);
            return;
        }
        if action == "show" || action == "toggle" {
            // Desktop activation lets Hype Flow own its setup and process lifecycle.
            if let Some(app) = crate::ui::flow_desktop() {
                if let Err(error) = app.launch(&[], None::<&gio::AppLaunchContext>) {
                    self.show_error(&error.to_string());
                }
            } else {
                self.show_error(
                    "Hype Flow is unavailable. Install its desktop application to start dictation.",
                );
            }
        }
    }
    fn show_error(&self, text: &str) {
        for view in self.views.borrow().iter() {
            view.show_message(text);
        }
    }
    pub fn request_level(self: &Rc<Self>, brightness: bool, value: f64, device: Option<Backlight>) {
        let queue = if brightness {
            &self.brightness_writes
        } else {
            &self.volume_writes
        };
        let request = queue.borrow_mut().request((value, device));
        if let Some(request) = request {
            self.start_level_write(brightness, request);
        }
    }
    fn start_level_write(self: &Rc<Self>, brightness: bool, (value, device): LevelRequest) {
        let weak = Rc::downgrade(self);
        let audio = self.audio.clone();
        glib::spawn_future_local(async move {
            let result = if brightness {
                match device {
                    Some(device) => crate::services::set_brightness(&device, value).await,
                    None => Err("No backlight device".into()),
                }
            } else {
                audio.volume(Some(value)).await.map(|_| ())
            };
            let Some(this) = weak.upgrade() else {
                return;
            };
            if let Err(error) = &result {
                this.show_error(error);
            }
            let queue = if brightness {
                &this.brightness_writes
            } else {
                &this.volume_writes
            };
            let next = queue.borrow_mut().finish();
            if let Some(request) = next {
                this.start_level_write(brightness, request);
            } else if result.is_ok() {
                for view in this.views.borrow().iter() {
                    view.sync_level(brightness, value);
                }
            }
        });
    }
    pub fn toggle(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let output = gio::spawn_blocking(crate::services::focused_output)
                .await
                .ok()
                .and_then(|result| result.ok())
                .flatten();
            let Some(this) = weak.upgrade() else {
                return;
            };
            let views = this.views.borrow();
            let selected = output
                .and_then(|name| {
                    views
                        .iter()
                        .find(|view| view.monitor.connector().as_deref() == Some(&name))
                })
                .or_else(|| views.first());
            if let Some(view) = selected {
                view.pin(true);
            }
        });
    }
}

fn prune_shelf() {
    glib::spawn_future_local(async {
        let result = gio::spawn_blocking(|| {
            crate::shelf::prune(&crate::shelf::cache_dir(), std::time::SystemTime::now())
        })
        .await;
        if let Ok(Err(error)) = result {
            eprintln!("Could not clean up the notch shelf: {error}");
        }
    });
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(source) = self.clock.take() {
            source.remove();
        }
        if let Some(source) = self.shelf_source.take() {
            source.remove();
        }
        if let Some(source) = self.weather_source.take() {
            source.remove();
        }
        if let Some(watch) = self.flow_watch.take() {
            gio::bus_unwatch_name(watch);
        }
        self.bus_signal.take();
        for view in self.views.get_mut().drain(..) {
            view.window.destroy();
        }
        self.hold.take();
    }
}
