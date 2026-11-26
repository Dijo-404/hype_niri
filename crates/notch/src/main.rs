#[path = "../../native/src/backlight.rs"]
mod backlight;
mod controller;
mod model;
mod morph;
#[path = "../../native/src/mpris.rs"]
mod mpris;
#[allow(dead_code)]
#[path = "../../native/src/pulse.rs"]
mod pulse;
mod services;
mod shelf;
mod surface;
mod ui;

use gtk::{gio, glib, prelude::*};
use std::{cell::RefCell, rc::Rc};

fn load_style() -> Result<(), String> {
    let provider: gtk::CssProvider = glib::Object::new();
    let errors = Rc::new(RefCell::new(Vec::new()));
    let found = errors.clone();
    provider.connect_parsing_error(move |_, _, error| {
        found.borrow_mut().push(error.to_string());
    });
    provider.load_from_string(include_str!("../style.css"));
    if !errors.borrow().is_empty() {
        return Err(errors.borrow().join("\n"));
    }
    if gtk::is_initialized_main_thread()
        && let Some(display) = gtk::gdk::Display::default()
    {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    Ok(())
}
fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 2
        || args
            .get(1)
            .is_some_and(|arg| !["--check", "--toggle", "--quit"].contains(&arg.as_str()))
    {
        eprintln!("Usage: hype-notch [--toggle|--quit|--check]");
        return glib::ExitCode::from(2);
    }
    if args.get(1).is_some_and(|arg| arg == "--check") {
        return match load_style() {
            Ok(()) => {
                println!("Rust GTK4, layer-shell and embedded style ready");
                glib::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Notch style: {error}");
                glib::ExitCode::FAILURE
            }
        };
    }
    if gtk::init().is_err() || !gtk4_layer_shell::is_supported() {
        eprintln!("Niri Notch requires a Wayland session with layer-shell support.");
        return glib::ExitCode::FAILURE;
    }
    if let Err(error) = load_style() {
        eprintln!("Notch style: {error}");
        return glib::ExitCode::FAILURE;
    }
    let app = gtk::Application::new(
        Some("dev.hype_niri.Notch"),
        gio::ApplicationFlags::HANDLES_COMMAND_LINE,
    );
    let state: Rc<RefCell<Option<Rc<controller::Controller>>>> = Rc::new(RefCell::new(None));
    let active = state.clone();
    app.connect_activate(move |app| {
        if active.borrow().is_none() {
            *active.borrow_mut() = Some(controller::Controller::new(app));
        }
    });
    let commands = state.clone();
    app.connect_command_line(move |app, command| {
        let args = command.arguments();
        match args.get(1).and_then(|s| s.to_str()) {
            Some("--quit") => {
                app.quit();
                glib::ExitCode::SUCCESS
            }
            None | Some("--toggle") => {
                app.activate();
                if let Some(controller) = commands.borrow().as_ref()
                    && args.len() == 2
                {
                    controller.toggle();
                }
                glib::ExitCode::SUCCESS
            }
            _ => glib::ExitCode::from(2),
        }
    });
    app.connect_shutdown(move |_| {
        state.take();
    });
    let weak = app.downgrade();
    let term = glib_unix::unix_signal_add_local(15, move || {
        if let Some(app) = weak.upgrade() {
            app.quit();
        }
        glib::ControlFlow::Continue
    });
    let weak = app.downgrade();
    let interrupt = glib_unix::unix_signal_add_local(2, move || {
        if let Some(app) = weak.upgrade() {
            app.quit();
        }
        glib::ControlFlow::Continue
    });
    let result = app.run_with_args(&args);
    term.remove();
    interrupt.remove();
    result
}
