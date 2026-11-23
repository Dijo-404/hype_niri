pub mod bus;
pub mod caffeine;
pub mod dialog;
pub mod ipc;
pub mod network;
pub mod notify;
pub mod runtime;
pub mod session;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub fn err(message: impl Into<String>) -> Box<dyn std::error::Error> {
    std::io::Error::other(message.into()).into()
}

pub mod controls;

pub mod temperature;

pub mod display;

pub mod wallpaper;

pub mod drives;

pub mod stealth;

pub mod stealth_client;
