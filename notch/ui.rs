use crate::{
    controller::Controller,
    model::{FocusTimer, Stopwatch, stopwatch_text},
    morph::Morph,
    services::{Backlight, Media},
    shelf::{CachedFile, cache_dir, thumbnail},
    surface::rounded_region,
};
use gtk::{cairo, gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Hidden,
    Peek,
    Shelf,
    Open,
}

pub struct Level {
    pub row: gtk::Box,
    pub scale: gtk::Scale,
    value: gtk::Label,
    syncing: Cell<bool>,
    device: RefCell<Option<Backlight>>,
}
impl Level {
    fn new(icon: &str, name: &str, min: f64) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::from_icon_name(icon));
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_focus_on_click(false);
        scale.update_property(&[gtk::accessible::Property::Label(name)]);
        row.append(&scale);
        let value = label("", "level-value");
        value.set_xalign(1.0);
        value.set_width_chars(4);
        row.append(&value);
        Self {
            row,
            scale,
            value,
            syncing: Cell::new(false),
            device: RefCell::new(None),
        }
    }
    fn sync(&self, value: f64) {
        self.syncing.set(true);
        self.scale.set_value(value);
        self.value
            .set_text(&format!("{}%", self.scale.value() as i32));
        self.syncing.set(false);
        self.row.set_visible(true);
    }
}

pub struct View {
    shared: Weak<Controller>,
    pub monitor: gdk::Monitor,
    pub window: gtk::ApplicationWindow,
    pub morph: Morph,
    pub surface: gtk::Box,
    pub header: gtk::Button,
    pub body: gtk::Box,
    pub search: gtk::Entry,
    pub results: gtk::ListBox,
    shelf: gtk::Box,
    shelf_open: gtk::Button,
    shelf_controls: gtk::Button,
    shelf_message: gtk::Label,
    shelf_all: gtk::Label,
    shelf_scroll: gtk::ScrolledWindow,
    shelf_items: gtk::Box,
    shelf_dirty: Cell<bool>,
    dragging_files: Cell<bool>,
    search_apps: RefCell<Vec<Option<gio::AppInfo>>>,
    pub state: Cell<State>,
    pub pinned: Cell<bool>,
    close_source: RefCell<Option<glib::SourceId>>,
    open_source: RefCell<Option<glib::SourceId>>,
    geometry_tick: RefCell<Option<gtk::TickCallbackId>>,
    applied_input: RefCell<Option<cairo::Region>>,
    pub summary: gtk::Label,
    compact: gtk::Label,
    media_icon: gtk::Image,
    header_wave: gtk::DrawingArea,
    card_wave: gtk::DrawingArea,
    wave_tick: RefCell<Option<gtk::TickCallbackId>>,
    media_card: gtk::Box,
    pub title: gtk::Label,
    artist: gtk::Label,
    previous: gtk::Button,
    pub play: gtk::Button,
    next: gtk::Button,
    phase: gtk::Label,
    pub countdown: gtk::Label,
    timer_button: gtk::Button,
    ring: gtk::DrawingArea,
    pub watch: gtk::Label,
    watch_state: gtk::Label,
    watch_button: gtk::Button,
    watch_ring: gtk::DrawingArea,
    weather_icon: gtk::Image,
    weather_temp: gtk::Label,
    forecast_icon: gtk::Image,
    forecast_temp: gtk::Label,
    forecast_condition: gtk::Label,
    forecast_detail: gtk::Label,
    forecast_place: gtk::Label,
    pub flow: gtk::Revealer,
    pub flow_button: gtk::Button,
    flow_meter: gtk::DrawingArea,
    flow_label: gtk::Label,
    flow_card: gtk::Box,
    flow_status: gtk::Label,
    flow_toggle: gtk::Button,
    flow_cancel: gtk::Button,
    flow_copy: gtk::Button,
    flow_show: gtk::Button,
    pub volume: Level,
    pub brightness: Level,
    message: gtk::Label,
}
fn label(text: &str, class: &str) -> gtk::Label {
    let widget = gtk::Label::new(Some(text));
    widget.set_xalign(0.0);
    widget.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if !class.is_empty() {
        widget.add_css_class(class);
    }
    widget
}
fn button(icon: &str, name: &str) -> gtk::Button {
    let widget = gtk::Button::from_icon_name(icon);
    widget.set_focus_on_click(false);
    widget.set_valign(gtk::Align::Center);
    set_button(&widget, icon, name);
    widget
}
fn set_button(widget: &gtk::Button, icon: &str, name: &str) {
    widget.set_icon_name(icon);
    widget.set_tooltip_text(Some(name));
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}
fn card(name: &str) -> gtk::Box {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 6);
    widget.set_widget_name(name);
    widget.add_css_class("card");
    widget
}
fn wave() -> gtk::DrawingArea {
    let wave = gtk::DrawingArea::new();
    wave.set_content_width(18);
    wave.set_content_height(16);
    wave.set_valign(gtk::Align::Center);
    wave.add_css_class("wave");
    wave.set_draw_func(|wave, cr, _, height| {
        let color = wave.color();
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            color.alpha().into(),
        );
        let now = if wave.settings().is_gtk_enable_animations() {
            wave.frame_clock()
                .map(|clock| clock.frame_time() as f64 / 1e6)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        cr.set_line_width(3.0);
        cr.set_line_cap(cairo::LineCap::Round);
        for i in 0..4 {
            let phase = (now * (1.1 + 0.25 * f64::from(i)) + 0.31 * f64::from(i)).fract();
            let level = if phase < 0.5 {
                phase * 2.0
            } else {
                2.0 - phase * 2.0
            };
            let level = level * level * (3.0 - 2.0 * level);
            let bar = f64::from(height - 4) * (0.3 + 0.7 * level);
            cr.move_to(2.5 + f64::from(i) * 5.0, (f64::from(height) - bar) / 2.0);
            cr.line_to(2.5 + f64::from(i) * 5.0, (f64::from(height) + bar) / 2.0);
        }
        let _ = cr.stroke();
    });
    wave
}
fn timer_page(
    initial: &str,
    state: &str,
    start: &str,
    left: &impl IsA<gtk::Widget>,
    reset: &gtk::Button,
) -> (
    gtk::Box,
    gtk::DrawingArea,
    gtk::Label,
    gtk::Label,
    gtk::Button,
) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let dial = gtk::Overlay::new();
    dial.set_halign(gtk::Align::Center);
    let ring = gtk::DrawingArea::new();
    ring.set_content_width(96);
    ring.set_content_height(96);
    ring.add_css_class("ring");
    dial.set_child(Some(&ring));
    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_halign(gtk::Align::Center);
    text.set_valign(gtk::Align::Center);
    let value = label(initial, "ring-time");
    value.set_xalign(0.5);
    let state = label(state, "muted");
    state.set_xalign(0.5);
    text.append(&value);
    text.append(&state);
    dial.add_overlay(&text);
    page.append(&dial);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.set_halign(gtk::Align::Center);
    let start = button("media-playback-start-symbolic", start);
    start.add_css_class("primary");
    start.add_css_class("large");
    row.append(left);
    row.append(&start);
    row.append(reset);
    page.append(&row);
    (page, ring, value, state, start)
}
impl View {
    pub fn new(shared: &Rc<Controller>, monitor: &gdk::Monitor) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(&shared.app)
            .title("Niri Notch")
            .decorated(false)
            .resizable(false)
            .default_width(400)
            .default_height(820)
            .build();
        window.set_widget_name("niri-notch");
        window.set_size_request(400, 820);
        window.init_layer_shell();
        window.set_namespace(Some("hype-notch"));
        window.set_monitor(Some(monitor));
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Top, true);
        // Ignore Waybar's exclusive zone; 48 logical pixels clears this repo's bar.
        window.set_exclusive_zone(-1);
        window.set_margin(Edge::Top, 48);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        let stage = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stage.set_valign(gtk::Align::Start);
        window.set_child(Some(&stage));
        let flow = gtk::Revealer::new();
        flow.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        flow.set_transition_duration(150);
        flow.set_halign(gtk::Align::Center);
        let flow_button = gtk::Button::new();
        flow_button.set_focus_on_click(false);
        flow_button.set_widget_name("notch-dictation");
        flow_button.set_size_request(400, -1);
        // Widget margins sit outside the allocation, so the blur region matches.
        flow_button.set_margin_top(2);
        flow_button.set_margin_bottom(8);
        let capsule = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let voice = gtk::DrawingArea::new();
        voice.set_content_width(30);
        voice.set_content_height(30);
        voice.set_draw_func(|_, cr, _, _| {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.13);
            cr.arc(15.0, 15.0, 15.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(2.0);
            cr.set_line_cap(cairo::LineCap::Round);
            for (x, h) in [(9.0, 6.0), (15.0, 14.0), (21.0, 9.0)] {
                cr.move_to(x, 15.0 - h / 2.0);
                cr.line_to(x, 15.0 + h / 2.0);
            }
            let _ = cr.stroke();
        });
        let flow_label = label("", "");
        flow_label.set_hexpand(true);
        let flow_meter = gtk::DrawingArea::new();
        flow_meter.set_content_width(40);
        flow_meter.set_content_height(30);
        flow_meter.set_hexpand(true);
        capsule.append(&voice);
        capsule.append(&flow_label);
        capsule.append(&flow_meter);
        flow_button.set_child(Some(&capsule));
        flow.set_child(Some(&flow_button));
        stage.append(&flow);
        let surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
        surface.set_widget_name("notch-surface");
        surface.set_focusable(true);
        let header = gtk::Button::new();
        header.set_widget_name("notch-header");
        // Mod+N and Escape cover it; as the first focusable widget it would
        // otherwise take initial focus and draw a ring once a key is pressed.
        header.set_can_focus(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let media_icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        let header_wave = wave();
        let summary = label("No media playing", "");
        summary.set_max_width_chars(25);
        summary.set_hexpand(true);
        let compact = label("", "chip");
        compact.set_visible(false);
        let glance = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        glance.add_css_class("glance");
        let weather_icon = gtk::Image::from_icon_name("weather-overcast-symbolic");
        let weather_temp = label("--", "");
        glance.append(&weather_icon);
        glance.append(&weather_temp);
        row.append(&media_icon);
        row.append(&header_wave);
        row.append(&summary);
        row.append(&compact);
        row.append(&glance);
        header.set_child(Some(&row));
        surface.append(&header);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.set_widget_name("notch-body");
        surface.append(&body);
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let search = gtk::Entry::new();
        search.set_widget_name("notch-search");
        search.set_placeholder_text(Some("Search apps or the web"));
        search.set_icon_from_icon_name(
            gtk::EntryIconPosition::Primary,
            Some("system-search-symbolic"),
        );
        search.update_property(&[gtk::accessible::Property::Label("Quick search")]);
        let results = gtk::ListBox::new();
        results.set_widget_name("notch-results");
        results.set_visible(false);
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search.set_hexpand(true);
        search_row.append(&search);
        let shelf_open = button("folder-symbolic", "Open file shelf");
        shelf_open.set_visible(false);
        search_row.append(&shelf_open);
        search_box.append(&search_row);
        search_box.append(&results);
        body.append(&search_box);
        let shelf = gtk::Box::new(gtk::Orientation::Vertical, 8);
        shelf.set_widget_name("notch-shelf");
        shelf.set_visible(false);
        shelf.set_focusable(true);
        let shelf_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        shelf_heading.append(&gtk::Image::from_icon_name("folder-symbolic"));
        let shelf_title = label("File shelf", "strong");
        shelf_title.set_hexpand(true);
        shelf_heading.append(&shelf_title);
        let shelf_all = label("", "shelf-item");
        shelf_all.set_visible(false);
        shelf_all.set_tooltip_text(Some("Drag all files out of the shelf"));
        shelf_heading.append(&shelf_all);
        let shelf_controls = button("view-grid-symbolic", "Open notch controls");
        shelf_heading.append(&shelf_controls);
        shelf.append(&shelf_heading);
        let shelf_message = label("Drop files or paste a screenshot", "muted");
        shelf_message.add_css_class("shelf-drop-area");
        shelf_message.set_xalign(0.5);
        shelf.append(&shelf_message);
        let shelf_items = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let shelf_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&shelf_items)
            .visible(false)
            .build();
        shelf.append(&shelf_scroll);
        surface.append(&shelf);
        let flow_card = card("notch-dictation-controls");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let flow_toggle = button("audio-input-microphone-symbolic", "Start dictation");
        flow_toggle.add_css_class("primary");
        let flow_cancel = button("process-stop-symbolic", "Cancel dictation");
        let flow_copy = button("edit-copy-symbolic", "Copy last transcript");
        let flow_show = button("preferences-system-symbolic", "Hype Flow settings");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_valign(gtk::Align::Center);
        text.set_margin_start(4);
        text.set_hexpand(true);
        text.append(&label("Hype Flow", "strong"));
        let flow_status = label("Open Hype Flow to start dictation", "muted");
        text.append(&flow_status);
        row.append(&flow_toggle);
        row.append(&text);
        row.append(&flow_cancel);
        row.append(&flow_copy);
        row.append(&flow_show);
        flow_card.append(&row);
        body.append(&flow_card);
        let weather = card("notch-weather");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let forecast_icon = gtk::Image::from_icon_name("weather-overcast-symbolic");
        forecast_icon.set_pixel_size(28);
        let forecast_temp = label("--", "temp");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_valign(gtk::Align::Center);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let forecast_condition = label("Loading weather", "strong");
        forecast_condition.set_hexpand(true);
        let forecast_place = label("", "muted");
        forecast_place.set_xalign(1.0);
        line.append(&forecast_condition);
        line.append(&forecast_place);
        let forecast_detail = label("", "muted");
        text.append(&line);
        text.append(&forecast_detail);
        text.set_hexpand(true);
        row.append(&forecast_icon);
        row.append(&forecast_temp);
        row.append(&text);
        weather.append(&row);
        body.append(&weather);
        let levels = card("notch-levels");
        let volume = Level::new("audio-volume-high-symbolic", "Volume", 0.0);
        let brightness = Level::new("display-brightness-symbolic", "Brightness", 1.0);
        levels.append(&volume.row);
        levels.append(&brightness.row);
        body.append(&levels);
        let media_card = card("notch-media");
        let title = label("Nothing playing", "track-title");
        title.set_max_width_chars(35);
        title.set_hexpand(true);
        let artist = label("Start a song or video in your media player", "muted");
        artist.set_max_width_chars(42);
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let card_wave = wave();
        heading.append(&title);
        heading.append(&card_wave);
        media_card.append(&heading);
        media_card.append(&artist);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(gtk::Align::Center);
        let previous = button("media-skip-backward-symbolic", "Previous track");
        let play = button("media-playback-start-symbolic", "Play");
        play.add_css_class("primary");
        let next = button("media-skip-forward-symbolic", "Next track");
        row.append(&previous);
        row.append(&play);
        row.append(&next);
        media_card.append(&row);
        body.append(&media_card);
        let timers = card("notch-timers");
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        stack.set_transition_duration(150);
        let skip = button("media-skip-forward-symbolic", "Skip to next phase");
        let reset = button("view-refresh-symbolic", "Reset focus timer");
        let (page, ring, countdown, phase, timer_button) =
            timer_page("25:00", "Focus", "Start focus timer", &skip, &reset);
        stack.add_titled(&page, Some("pomodoro"), "Pomodoro");
        // Holds the left slot open so the stopwatch controls stay centred.
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(34, 34);
        let watch_reset = button("view-refresh-symbolic", "Reset stopwatch");
        let (page, watch_ring, watch, watch_state, watch_button) = timer_page(
            "00:00.0",
            "Stopwatch",
            "Start stopwatch",
            &spacer,
            &watch_reset,
        );
        stack.add_titled(&page, Some("stopwatch"), "Stopwatch");
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        timers.append(&switcher);
        timers.append(&stack);
        body.append(&timers);
        let message = label("", "muted");
        message.set_wrap(true);
        message.set_ellipsize(gtk::pango::EllipsizeMode::None);
        message.set_max_width_chars(40);
        message.set_visible(false);
        body.append(&message);
        let morph = Morph::new(&surface);
        stage.append(&morph);
        body.set_visible(false);
        body.set_child_visible(false);
        let this = Rc::new(Self {
            shared: Rc::downgrade(shared),
            monitor: monitor.clone(),
            window,
            morph,
            surface,
            header,
            body,
            search,
            results,
            shelf,
            shelf_open,
            shelf_controls,
            shelf_message,
            shelf_all,
            shelf_scroll,
            shelf_items,
            shelf_dirty: Cell::new(false),
            dragging_files: Cell::new(false),
            search_apps: RefCell::new(Vec::new()),
            state: Cell::new(State::Hidden),
            pinned: Cell::new(false),
            close_source: RefCell::new(None),
            open_source: RefCell::new(None),
            geometry_tick: RefCell::new(None),
            applied_input: RefCell::new(None),
            summary,
            compact,
            media_icon,
            header_wave,
            card_wave,
            wave_tick: RefCell::new(None),
            media_card,
            title,
            artist,
            previous,
            play,
            next,
            phase,
            countdown,
            timer_button,
            ring,
            watch,
            watch_state,
            watch_button,
            watch_ring,
            weather_icon,
            weather_temp,
            forecast_icon,
            forecast_temp,
            forecast_condition,
            forecast_detail,
            forecast_place,
            flow,
            flow_button,
            flow_meter,
            flow_label,
            flow_card,
            flow_status,
            flow_toggle,
            flow_cancel,
            flow_copy,
            flow_show,
            volume,
            brightness,
            message,
        });
        this.wire(&skip, &reset, &watch_reset);
        this.refresh_levels();
        this
    }
    fn wire(self: &Rc<Self>, skip: &gtk::Button, reset: &gtk::Button, watch_reset: &gtk::Button) {
        self.wire_shelf();
        let weak = Rc::downgrade(self);
        self.header.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.pin(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.shelf_controls.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.pin(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.shelf_open.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.open_shelf();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_map(move |_| {
            if let Some(this) = weak.upgrade() {
                this.applied_input.take();
                this.retarget();
                this.sync_geometry();
            }
        });
        let weak = Rc::downgrade(self);
        self.window.connect_is_active_notify(move |window| {
            if !window.is_active()
                && let Some(this) = weak.upgrade()
                && !this.dragging_files.get()
                && !this.shelf.has_css_class("drop-active")
                && (this.pinned.get() || window.keyboard_mode() == KeyboardMode::Exclusive)
            {
                this.collapse();
            }
        });
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.hover();
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.hover();
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.leave();
            }
        });
        self.window.add_controller(motion);
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        key.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if this.state.get() != State::Hidden
                && matches!(key, gdk::Key::v | gdk::Key::V)
                && modifiers & gtk::accelerator_get_default_mod_mask()
                    == gdk::ModifierType::CONTROL_MASK
                && this
                    .window
                    .clipboard()
                    .formats()
                    .contains_type(gdk::Texture::static_type())
            {
                this.paste_to_shelf();
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::Escape {
                if this.search.text().is_empty() {
                    this.collapse();
                } else {
                    this.search.set_text("");
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(key);
        let weak = Rc::downgrade(self);
        self.search.connect_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.search_changed();
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            if let Some(this) = weak.upgrade()
                && let Some(row) = this.results.selected_row()
            {
                this.launch_row(&row);
            }
        });
        let weak = Rc::downgrade(self);
        self.results.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.launch_row(row);
            }
        });
        let key = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        key.connect_key_pressed(move |_, key, _, _| {
            let step = if key == gdk::Key::Down {
                1
            } else if key == gdk::Key::Up {
                -1
            } else {
                return glib::Propagation::Proceed;
            };
            if let Some(this) = weak.upgrade() {
                let count = this.search_apps.borrow().len() as i32;
                if count > 0 {
                    let index = this
                        .results
                        .selected_row()
                        .map(|row| row.index() + step)
                        .unwrap_or(0)
                        .rem_euclid(count);
                    this.results
                        .select_row(this.results.row_at_index(index).as_ref());
                }
            }
            glib::Propagation::Stop
        });
        self.search.add_controller(key);
        for (button, action) in [
            (&self.previous, "Previous"),
            (&self.play, "PlayPause"),
            (&self.next, "Next"),
        ] {
            let shared = self.shared.clone();
            button.connect_clicked(move |_| {
                if let Some(shared) = shared.upgrade() {
                    shared.media_action(action);
                }
            });
        }
        for (button, action) in [
            (&self.flow_toggle, "toggle"),
            (&self.flow_cancel, "cancel"),
            (&self.flow_copy, "copy-last"),
            (&self.flow_show, "show"),
        ] {
            let shared = self.shared.clone();
            button.connect_clicked(move |_| {
                if let Some(shared) = shared.upgrade() {
                    shared.flow_action(action);
                }
            });
        }
        let shared = self.shared.clone();
        self.flow_button.connect_clicked(move |_| {
            if let Some(shared) = shared.upgrade() {
                let recording = shared.flow.borrow().recording();
                shared.flow_action(if recording { "toggle" } else { "show" });
            }
        });
        let click = gtk::GestureClick::new();
        click.set_button(3);
        let shared = self.shared.clone();
        click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(shared) = shared.upgrade() {
                shared.flow_action("cancel");
            }
        });
        self.flow_button.add_controller(click);
        let shared = self.shared.clone();
        self.flow_meter.set_draw_func(move |_, cr, width, height| {
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let levels = shared.flow_levels.borrow();
            let count = (width as usize / 5).min(64);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(2.5);
            cr.set_line_cap(cairo::LineCap::Round);
            for i in 0..count {
                let bar = 3.0 + 16.5 * levels[64 - count + i];
                let x = (width as f64 - count as f64 * 5.0) / 2.0 + i as f64 * 5.0 + 2.5;
                cr.move_to(x, (f64::from(height) - bar) / 2.0);
                cr.line_to(x, (f64::from(height) + bar) / 2.0);
            }
            let _ = cr.stroke();
        });
        for (button, operation) in [
            (&self.timer_button, 0),
            (skip, 1),
            (reset, 2),
            (&self.watch_button, 3),
            (watch_reset, 4),
        ] {
            let shared = self.shared.clone();
            button.connect_clicked(move |_| {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let now = glib::monotonic_time();
                match operation {
                    0 => {
                        let completed = shared.timer.borrow_mut().toggle(now);
                        shared.notify(completed);
                    }
                    1 => shared.timer.borrow_mut().skip(now),
                    2 => *shared.timer.borrow_mut() = FocusTimer::default(),
                    3 => shared.stopwatch.borrow_mut().toggle(now),
                    _ => *shared.stopwatch.borrow_mut() = Stopwatch::default(),
                }
                shared.sync_clocks();
            });
        }
        for (ring, focus) in [(&self.ring, true), (&self.watch_ring, false)] {
            let shared = self.shared.clone();
            ring.set_draw_func(move |ring, cr, width, height| {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let timer = shared.timer.borrow();
                let fraction = if focus {
                    f64::from(timer.remaining) / f64::from(timer.duration())
                } else {
                    (shared.stopwatch.borrow().elapsed(glib::monotonic_time()) % 60_000_000) as f64
                        / 60_000_000.0
                };
                let color = ring.color();
                let (red, green, blue) = (
                    f64::from(color.red()),
                    f64::from(color.green()),
                    f64::from(color.blue()),
                );
                let (x, y, radius) = (
                    f64::from(width) / 2.0,
                    f64::from(height) / 2.0,
                    f64::from(width.min(height)) / 2.0 - 3.0,
                );
                cr.set_line_width(3.0);
                cr.set_line_cap(cairo::LineCap::Round);
                cr.set_source_rgba(red, green, blue, 0.15);
                cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
                let _ = cr.stroke();
                if fraction > 0.0 {
                    cr.set_source_rgba(red, green, blue, 1.0);
                    cr.arc(
                        x,
                        y,
                        radius,
                        -std::f64::consts::FRAC_PI_2,
                        -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * fraction,
                    );
                    let _ = cr.stroke();
                }
            });
        }
        for (scale, brightness) in [(&self.volume.scale, false), (&self.brightness.scale, true)] {
            let weak = Rc::downgrade(self);
            scale.connect_value_changed(move |scale| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let level = if brightness {
                    &this.brightness
                } else {
                    &this.volume
                };
                level.value.set_text(&format!("{}%", scale.value() as i32));
                if level.syncing.get() {
                    return;
                }
                if let Some(shared) = this.shared.upgrade() {
                    shared.request_level(brightness, scale.value(), level.device.borrow().clone());
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.flow.connect_child_revealed_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.retarget();
                this.sync_geometry();
            }
        });
        let weak = Rc::downgrade(self);
        self.window
            .settings()
            .connect_gtk_enable_animations_notify(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.refresh_media_wave();
                    this.retarget();
                    this.sync_geometry();
                }
            });
    }
    fn wire_shelf(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        // An outbound drag over another notch view is still a drag from this shelf.
        target.connect_accept(|target, drop| {
            drop.drag().is_none()
                && target
                    .formats()
                    .is_some_and(|formats| drop.formats().match_(&formats))
        });
        let weak = Rc::downgrade(self);
        target.connect_enter(move |_, _, _| {
            let Some(this) = weak.upgrade() else {
                return gdk::DragAction::empty();
            };
            this.shelf.add_css_class("drop-active");
            this.window.set_keyboard_mode(KeyboardMode::None);
            this.open_shelf();
            gdk::DragAction::COPY
        });
        let weak = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.shelf.remove_css_class("drop-active");
                this.leave();
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(this), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
                return false;
            };
            let files = files.files();
            let paths: Vec<_> = files
                .iter()
                .filter(|file| file.is_native())
                .filter_map(gio::File::path)
                .collect();
            if paths.is_empty() {
                this.show_shelf_message("Drop a local file");
                return false;
            }
            let weak = Rc::downgrade(&this);
            let shared = this.shared.clone();
            let root = cache_dir();
            let count = files.len();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || {
                    paths
                        .into_iter()
                        .filter_map(|path| CachedFile::copy(&root, path).ok())
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                let failed = result.len() != count;
                if let Some(shared) = shared.upgrade() {
                    shared.store_files(result);
                }
                if failed && let Some(this) = weak.upgrade() {
                    this.show_shelf_message("Some files could not be stored");
                }
            });
            true
        });
        // Attach to the window so the collapsed pill can receive a file drag.
        self.window.add_controller(target);
        let shared = self.shared.clone();
        self.wire_file_drag(&self.shelf_all, move || {
            shared
                .upgrade()
                .map_or_else(Vec::new, |shared| shared.shelf_files())
        });
    }
    fn paste_to_shelf(self: &Rc<Self>) {
        let clipboard = self.window.clipboard();
        let weak = Rc::downgrade(self);
        let shared = self.shared.clone();
        let root = cache_dir();
        glib::spawn_future_local(async move {
            let Ok(Some(texture)) = clipboard.read_texture_future().await else {
                if let Some(this) = weak.upgrade()
                    && this.state.get() == State::Shelf
                {
                    this.show_shelf_message("Copy a screenshot first");
                }
                return;
            };
            if let Some(this) = weak.upgrade()
                && this.state.get() != State::Hidden
            {
                this.open_shelf();
            }
            let result = gio::spawn_blocking(move || CachedFile::screenshot(&root, texture)).await;
            if let Ok(Ok(file)) = result {
                if let Some(shared) = shared.upgrade() {
                    shared.store_files(vec![file]);
                }
            } else if let Some(this) = weak.upgrade() {
                this.show_shelf_message("Could not store screenshot");
            }
        });
    }
    fn open_shelf(self: &Rc<Self>) {
        self.pinned.set(false);
        self.set_state(State::Shelf);
        if self.window.keyboard_mode() == KeyboardMode::Exclusive {
            self.shelf.grab_focus();
        }
    }
    fn show_shelf_message(self: &Rc<Self>, text: &str) {
        self.shelf_message.set_text(text);
        self.shelf_message.set_visible(true);
        self.retarget();
        self.sync_geometry();
    }
    fn wire_file_drag(
        self: &Rc<Self>,
        widget: &impl IsA<gtk::Widget>,
        files: impl Fn() -> Vec<Rc<CachedFile>> + 'static,
    ) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY);
        source.connect_prepare(move |_, _, _| {
            let files: Vec<_> = files()
                .iter()
                .map(|file| gio::File::for_path(file.path()))
                .collect();
            (!files.is_empty()).then(|| {
                gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value())
            })
        });
        let weak = Rc::downgrade(self);
        source.connect_drag_begin(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.dragging_files.set(true);
                this.window.set_keyboard_mode(KeyboardMode::OnDemand);
            }
        });
        let weak = Rc::downgrade(self);
        source.connect_drag_end(move |_, _, _| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(this) = weak.upgrade() {
                    this.dragging_files.set(false);
                    if this.shelf_dirty.replace(false)
                        && let Some(shared) = this.shared.upgrade()
                    {
                        this.refresh_shelf(&shared.shelf_files());
                    }
                    this.leave();
                }
            });
        });
        widget.set_cursor_from_name(Some("grab"));
        widget.add_controller(source);
    }
    pub fn refresh_shelf(self: &Rc<Self>, files: &[Rc<CachedFile>]) {
        if self.dragging_files.get() {
            self.shelf_dirty.set(true);
            return;
        }
        self.shelf_message
            .set_text("Drop files or paste a screenshot");
        self.shelf_message.set_visible(files.is_empty());
        self.shelf_all
            .set_text(&format!("Drag all ({})", files.len()));
        self.shelf_all.set_visible(files.len() > 1);
        self.shelf_scroll.set_visible(!files.is_empty());
        self.shelf_open.set_visible(!files.is_empty());
        while let Some(child) = self.shelf_items.first_child() {
            self.shelf_items.remove(&child);
        }
        for file in files {
            let name = file.path().file_name().unwrap_or_default();
            let (content_type, _) = gio::content_type_guess(Some(file.path()), None);
            let icon = gio::content_type_get_symbolic_icon(&content_type);
            let stem = file.path().file_stem().unwrap_or(name);
            let item = gtk::Box::new(gtk::Orientation::Vertical, 4);
            item.add_css_class("shelf-item");
            item.set_size_request(62, -1);
            item.set_tooltip_text(Some(&name.to_string_lossy()));
            let image = gtk::Image::from_gicon(&icon);
            image.set_size_request(52, 38);
            item.append(&image);
            if content_type.starts_with("image/") {
                let path = gio::File::for_path(file.path());
                let image = image.downgrade();
                glib::spawn_future_local(async move {
                    if let Some(preview) = thumbnail(path).await
                        && let Some(image) = image.upgrade()
                        && let Some(item) = image.parent().and_downcast::<gtk::Box>()
                    {
                        let picture = gtk::Picture::for_paintable(&preview);
                        picture.set_content_fit(gtk::ContentFit::ScaleDown);
                        picture.set_size_request(52, 38);
                        item.prepend(&picture);
                        item.remove(&image);
                    }
                });
            }
            let text = label(&stem.to_string_lossy(), "");
            text.set_xalign(0.5);
            text.set_max_width_chars(7);
            text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            item.append(&text);
            let tile = gtk::Overlay::new();
            tile.set_child(Some(&item));
            let remove = button(
                "window-close-symbolic",
                &format!("Remove {} from shelf", name.to_string_lossy()),
            );
            remove.add_css_class("shelf-remove");
            remove.set_halign(gtk::Align::End);
            remove.set_valign(gtk::Align::Start);
            let path = file.path().to_path_buf();
            let shared = self.shared.clone();
            remove.connect_clicked(move |_| {
                if let Some(shared) = shared.upgrade() {
                    shared.remove_file(&path);
                }
            });
            tile.add_overlay(&remove);
            let file = file.clone();
            self.wire_file_drag(&item, move || vec![file.clone()]);
            self.shelf_items.append(&tile);
        }
        self.retarget();
        self.sync_geometry();
    }
    pub fn show_message(self: &Rc<Self>, text: &str) {
        self.message.set_text(text);
        self.message.set_visible(!text.is_empty());
        self.retarget();
        self.sync_geometry();
    }
    pub fn pin(self: &Rc<Self>, focus: bool) {
        self.pinned.set(!self.pinned.get());
        self.set_state(if self.pinned.get() {
            State::Open
        } else {
            State::Hidden
        });
        if self.pinned.get() && focus {
            // The shortcut and shelf button focus search when opening the controls.
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
            self.search.grab_focus();
        }
    }
    pub fn collapse(self: &Rc<Self>) {
        self.pinned.set(false);
        self.set_state(State::Hidden);
    }
    pub fn set_state(self: &Rc<Self>, state: State) {
        if let Some(source) = self.close_source.take() {
            source.remove();
        }
        if let Some(source) = self.open_source.take() {
            source.remove();
        }
        self.state.set(state);
        self.shelf.set_visible(state == State::Shelf);
        self.header.set_visible(state != State::Shelf);
        self.body.set_visible(state == State::Open);
        self.body.set_child_visible(state == State::Open);
        self.header
            .update_property(&[gtk::accessible::Property::Label(if state == State::Open {
                "Close notch controls"
            } else {
                "Open notch controls"
            })]);
        self.header
            .update_state(&[gtk::accessible::State::Expanded(Some(state == State::Open))]);
        if state == State::Open {
            self.refresh_levels();
        } else {
            self.search.set_text("");
            if state == State::Hidden {
                self.update_keyboard();
            }
        }
        self.retarget();
        self.sync_geometry();
    }
    fn update_keyboard(&self) {
        if self.state.get() != State::Open {
            let flow_visible = self
                .shared
                .upgrade()
                .is_some_and(|shared| shared.flow.borrow().visible);
            self.window.set_keyboard_mode(if flow_visible {
                KeyboardMode::None
            } else {
                KeyboardMode::OnDemand
            });
        }
    }
    fn hover(self: &Rc<Self>) {
        if let Some(source) = self.close_source.take() {
            source.remove();
        }
        if self.state.get() == State::Hidden {
            self.set_state(State::Peek);
        } else if self.state.get() == State::Peek
            && self.open_source.borrow().is_none()
            && !self.morph.animating()
        {
            let weak = Rc::downgrade(self);
            *self.open_source.borrow_mut() = Some(glib::timeout_add_local_once(
                Duration::from_millis(100),
                move || {
                    if let Some(this) = weak.upgrade() {
                        this.open_source.take();
                        this.set_state(State::Open);
                    }
                },
            ));
        }
        if self.window.keyboard_mode() != KeyboardMode::Exclusive
            && !self.dragging_files.get()
            && !self.shelf.has_css_class("drop-active")
        {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
            self.surface.grab_focus();
        }
    }
    fn leave(self: &Rc<Self>) {
        if self.pinned.get()
            || self.dragging_files.get()
            || self.shelf.has_css_class("drop-active")
            || !self.search.text().is_empty()
        {
            return;
        }
        if self.window.keyboard_mode() == KeyboardMode::Exclusive {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        if let Some(source) = self.open_source.take() {
            source.remove();
        }
        if let Some(source) = self.close_source.take() {
            source.remove();
        }
        let weak = Rc::downgrade(self);
        *self.close_source.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(250),
            move || {
                if let Some(this) = weak.upgrade() {
                    this.close_source.take();
                    this.collapse();
                }
            },
        ));
    }
    fn retarget(&self) {
        let header = self
            .header
            .measure(gtk::Orientation::Vertical, 400)
            .1
            .max(9);
        let flow_visible = self
            .shared
            .upgrade()
            .is_some_and(|shared| shared.flow.borrow().visible);
        let (width, height) = match self.state.get() {
            State::Hidden => {
                if flow_visible {
                    (0.0, 0.0)
                } else {
                    (76.0, 8.0)
                }
            }
            State::Peek => (400.0, f64::from(header)),
            State::Shelf | State::Open => (
                400.0,
                f64::from(self.surface.measure(gtk::Orientation::Vertical, 400).1),
            ),
        };
        self.morph.retarget(width, height, header);
    }
    fn sync_geometry(self: &Rc<Self>) {
        self.update_regions();
        if let Some(tick) = self.geometry_tick.take() {
            tick.remove();
        }
        let start = glib::monotonic_time();
        let weak = Rc::downgrade(self);
        *self.geometry_tick.borrow_mut() = Some(self.window.add_tick_callback(move |_, clock| {
            let Some(this) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            this.update_regions();
            if clock.frame_time() - start >= 200_000 {
                this.geometry_tick.take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        }));
    }
    fn update_regions(&self) {
        if let Some(surface) = self.window.surface() {
            let input = self.input_region();
            let mut applied = self.applied_input.borrow_mut();
            if applied.as_ref() != Some(&input) {
                surface.set_input_region(Some(&input));
                // Wayland applies input regions on commit, even when the contents are idle.
                self.morph.queue_draw();
                *applied = Some(input);
            }
        }
    }
    fn input_region(&self) -> cairo::Region {
        let input = cairo::Region::create();
        if let Some(bounds) = self.morph.compute_bounds(&self.window) {
            let (x, y, w, h) = (
                bounds.x().round() as i32,
                bounds.y().round() as i32,
                bounds.width().round() as i32,
                bounds.height().round() as i32,
            );
            if w > 1 && h > 1 {
                if self.state.get() == State::Hidden {
                    // Pad the thin pill so it is easy to hit.
                    let _ =
                        input.union_rectangle(&cairo::RectangleInt::new(x - 28, y, w + 56, h + 12));
                } else {
                    let _ = input.union(&rounded_region(x, y, w, h, 22));
                }
            }
        }
        if (self.flow.reveals_child() || self.flow.is_child_revealed())
            && let (Some(button), Some(clip)) = (
                self.flow_button.compute_bounds(&self.window),
                self.flow.compute_bounds(&self.window),
            )
        {
            let region = rounded_region(
                button.x().round() as i32,
                button.y().round() as i32,
                button.width().round() as i32,
                button.height().round() as i32,
                i32::MAX,
            );
            let _ = region.intersect_rectangle(&cairo::RectangleInt::new(
                clip.x().round() as i32,
                clip.y().round() as i32,
                clip.width().round() as i32,
                clip.height().round() as i32,
            ));
            let _ = input.union(&region);
        }
        input
    }
    pub fn refresh_clocks(&self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let timer = shared.timer.borrow();
        let watch = shared.stopwatch.borrow();
        let elapsed = watch.elapsed(glib::monotonic_time());
        let countdown = format!("{:02}:{:02}", timer.remaining / 60, timer.remaining % 60);
        self.countdown.set_text(&countdown);
        self.phase.set_text(timer.phase());
        for widget in [
            self.phase.upcast_ref::<gtk::Widget>(),
            self.ring.upcast_ref(),
        ] {
            if timer.on_break {
                widget.add_css_class("break");
            } else {
                widget.remove_css_class("break");
            }
        }
        set_button(
            &self.timer_button,
            if timer.running() {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            },
            &format!(
                "{} {} timer",
                if timer.running() { "Pause" } else { "Start" },
                timer.phase().to_lowercase()
            ),
        );
        self.watch.set_text(&stopwatch_text(elapsed));
        self.watch_state.set_text(if watch.running {
            "Running"
        } else if elapsed > 0 {
            "Paused"
        } else {
            "Stopwatch"
        });
        set_button(
            &self.watch_button,
            if watch.running {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            },
            if watch.running {
                "Pause stopwatch"
            } else {
                "Start stopwatch"
            },
        );
        let focus = timer.running() || timer.on_break || timer.remaining != 1500;
        let watch_text = stopwatch_text(elapsed);
        self.compact
            .set_text(if focus { &countdown } else { &watch_text });
        self.compact.set_visible(focus || elapsed > 0);
        self.ring.queue_draw();
        self.watch_ring.queue_draw();
    }
    pub fn refresh_weather(self: &Rc<Self>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        if let Some(weather) = shared.weather.borrow().as_ref() {
            self.weather_icon.set_icon_name(Some(weather.icon()));
            self.forecast_icon.set_icon_name(Some(weather.icon()));
            self.weather_temp.set_text(&weather.temperature);
            self.forecast_temp.set_text(&weather.temperature);
            self.forecast_condition.set_text(&weather.condition);
            self.forecast_detail.set_text(&weather.detail);
            self.forecast_place.set_text(&weather.place);
        } else if shared.weather_loaded.get() {
            self.weather_icon
                .set_icon_name(Some("weather-overcast-symbolic"));
            self.forecast_icon
                .set_icon_name(Some("weather-overcast-symbolic"));
            self.weather_temp.set_text("--");
            self.forecast_temp.set_text("--");
            self.forecast_condition.set_text("Weather unavailable");
            self.forecast_detail.set_text("Retrying in 30 minutes");
            self.forecast_place.set_text("");
        }
        self.retarget();
        self.sync_geometry();
    }
    pub fn refresh_media(self: &Rc<Self>, media: &Media) {
        self.title.set_text(if media.title.is_empty() {
            "Nothing playing"
        } else {
            &media.title
        });
        self.artist.set_text(if !media.artist.is_empty() {
            &media.artist
        } else if !media.name.is_empty() {
            &media.name
        } else {
            "Start a song or video in your media player"
        });
        self.summary.set_text(if media.active() {
            &media.title
        } else {
            "No media playing"
        });
        if media.active() {
            self.summary.remove_css_class("muted");
        } else {
            self.summary.add_css_class("muted");
        }
        self.media_card.set_visible(media.active());
        self.media_icon.set_visible(!media.playing);
        self.header_wave.set_visible(media.playing);
        self.card_wave.set_visible(media.playing);
        set_button(
            &self.play,
            if media.playing {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            },
            if media.playing { "Pause" } else { "Play" },
        );
        self.previous.set_sensitive(media.can_previous);
        self.next.set_sensitive(media.can_next);
        self.play.set_sensitive(if media.playing {
            media.can_pause
        } else {
            media.can_play
        });
        self.refresh_media_wave();
        self.retarget();
        self.sync_geometry();
    }
    fn refresh_media_wave(self: &Rc<Self>) {
        let animate =
            self.header_wave.is_visible() && self.window.settings().is_gtk_enable_animations();
        if !animate {
            if let Some(tick) = self.wave_tick.take() {
                tick.remove();
            }
            self.header_wave.queue_draw();
            self.card_wave.queue_draw();
            return;
        }
        if self.wave_tick.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        *self.wave_tick.borrow_mut() = Some(self.header_wave.add_tick_callback(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.header_wave.queue_draw();
                this.card_wave.queue_draw();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        }));
    }
    pub fn refresh_flow(self: &Rc<Self>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let state = shared.flow.borrow();
        let available = shared.flow_actions.borrow().is_some() || flow_desktop().is_some();
        self.flow_card.set_visible(available);
        let was_visible = self.flow.reveals_child();
        let visible = state.visible;
        self.flow.set_reveal_child(visible);
        self.flow_meter.set_visible(state.recording());
        self.flow_meter.queue_draw();
        self.flow_label.set_visible(!state.recording());
        let caption = match state.state.as_str() {
            "error" => "Needs attention",
            "done" => &state.message,
            "loading" => "Loading model",
            "delivering" => "Copying",
            _ => "Transcribing",
        };
        self.flow_label.set_text(caption);
        self.flow_status.set_text(&state.message);
        set_button(
            &self.flow_toggle,
            if state.recording() {
                "media-playback-stop-symbolic"
            } else {
                "audio-input-microphone-symbolic"
            },
            if state.recording() {
                "Finish dictation"
            } else {
                "Start dictation"
            },
        );
        let idle = matches!(state.state.as_str(), "idle" | "done" | "error");
        self.flow_toggle.set_sensitive(idle || state.recording());
        self.flow_cancel.set_sensitive(!idle);
        self.flow_copy.set_sensitive(
            shared
                .flow_actions
                .borrow()
                .as_ref()
                .is_some_and(|actions| actions.has_action("copy-last")),
        );
        let tip = if state.recording() {
            "Listening: click to finish, right-click to cancel"
        } else {
            &state.message
        };
        self.flow_button.set_tooltip_text(Some(tip));
        self.flow_button
            .update_property(&[gtk::accessible::Property::Label(tip)]);
        drop(state);
        if visible || was_visible {
            self.window.set_keyboard_mode(if visible {
                KeyboardMode::None
            } else {
                KeyboardMode::OnDemand
            });
        }
        self.retarget();
        self.sync_geometry();
    }
    fn refresh_levels(self: &Rc<Self>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        if !shared.volume_writes.borrow().busy() {
            let revision = shared.volume_writes.borrow().revision();
            let audio = shared.audio.clone();
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let result = audio.volume(None).await;
                if let Some(this) = weak.upgrade() {
                    this.volume_read_done(revision, result.ok());
                }
            });
        }
        if shared.brightness_writes.borrow().busy() {
            return;
        }
        match crate::services::read_backlight(std::path::Path::new("/sys/class/backlight")) {
            Ok(device) => {
                self.brightness.sync(device.percent);
                *self.brightness.device.borrow_mut() = Some(device);
            }
            Err(_) => self.brightness.row.set_visible(false),
        }
    }
    fn volume_read_done(self: &Rc<Self>, revision: u64, result: Option<f64>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        if revision != shared.volume_writes.borrow().revision()
            || shared.volume_writes.borrow().busy()
        {
            return;
        }
        match result {
            Some(value) => self.volume.sync(value),
            None => self.volume.row.set_visible(false),
        }
        self.retarget();
        self.sync_geometry();
    }
    pub fn sync_level(self: &Rc<Self>, brightness: bool, value: f64) {
        if brightness {
            self.brightness.sync(value);
        } else {
            self.volume.sync(value);
        }
        self.retarget();
        self.sync_geometry();
    }
    fn search_changed(self: &Rc<Self>) {
        while let Some(child) = self.results.first_child() {
            self.results.remove(&child);
        }
        self.search_apps.borrow_mut().clear();
        let query = self.search.text();
        self.results.set_visible(!query.is_empty());
        if !query.is_empty() {
            for group in gio_unix::DesktopAppInfo::search(&query) {
                for id in group {
                    if self.search_apps.borrow().len() >= 5 {
                        break;
                    }
                    if let Some(app) = gio_unix::DesktopAppInfo::new(&id)
                        && app.should_show()
                    {
                        let name = app.name();
                        let icon = app.icon();
                        self.add_result(Some(app.upcast()), &name, icon.as_ref());
                    }
                }
            }
            let web = if query.starts_with("https://") || query.starts_with("http://") {
                format!("Open {query}")
            } else {
                format!("Search the web for “{query}”")
            };
            self.add_result(None, &web, None);
            self.results
                .select_row(self.results.row_at_index(0).as_ref());
        }
        self.retarget();
        self.sync_geometry();
    }
    fn add_result(&self, app: Option<gio::AppInfo>, text: &str, icon: Option<&gio::Icon>) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let image = icon
            .map(gtk::Image::from_gicon)
            .unwrap_or_else(|| gtk::Image::from_icon_name("web-browser-symbolic"));
        image.set_pixel_size(22);
        let text = label(text, "");
        text.set_hexpand(true);
        row.append(&image);
        row.append(&text);
        self.results.append(&row);
        self.search_apps.borrow_mut().push(app);
    }
    fn launch_row(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let app = self
            .search_apps
            .borrow()
            .get(row.index() as usize)
            .cloned()
            .flatten();
        let context = gtk::prelude::WidgetExt::display(&self.window).app_launch_context();
        if let Some(app) = app {
            match app.launch(&[], Some(&context)) {
                Ok(()) => self.collapse(),
                Err(error) => self.show_message(&error.to_string()),
            }
        } else {
            let query = self.search.text();
            let uri = if query.starts_with("https://") || query.starts_with("http://") {
                query.to_string()
            } else {
                format!(
                    "https://duckduckgo.com/?q={}",
                    glib::uri_escape_string(&query, None::<&str>, false)
                )
            };
            let weak = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                let result =
                    gio::AppInfo::launch_default_for_uri_future(&uri, Some(&context)).await;
                if let Some(this) = weak.upgrade() {
                    match result {
                        Ok(()) => this.collapse(),
                        Err(error) => this.show_message(&error.to_string()),
                    }
                }
            });
        }
    }
}
pub fn flow_desktop() -> Option<gio_unix::DesktopAppInfo> {
    gio_unix::DesktopAppInfo::new("dev.hype.Flow.desktop")
        .or_else(|| gio_unix::DesktopAppInfo::new("hype-flow.desktop"))
}
impl Drop for View {
    fn drop(&mut self) {
        if let Some(source) = self.close_source.take() {
            source.remove();
        }
        if let Some(source) = self.open_source.take() {
            source.remove();
        }
        if let Some(tick) = self.geometry_tick.take() {
            tick.remove();
        }
        if let Some(tick) = self.wave_tick.take() {
            tick.remove();
        }
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
