use gtk::glib;
use hype_desktop::{Result, err, network, temperature};

async fn run() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("temperature") => temperature::run("temperature", &["watch".into()]).await,
        Some("hotspot") => network::watch_status().await,
        Some("--check") => Ok(()),
        _ => Err(err("Usage: hype-status temperature|hotspot")),
    }
}

fn main() {
    if let Err(error) = glib::MainContext::default().block_on(run()) {
        eprintln!("hype-status: {error}");
        std::process::exit(1);
    }
}
