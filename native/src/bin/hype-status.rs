use gtk::glib;
use hype_desktop::{Result, err, helpers, network, temperature};

async fn run() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("temperature") => temperature::run("temperature", &["watch".into()]).await,
        Some("hotspot") => network::watch_status().await,
        Some("warp-indicator") => {
            helpers::warp_indicator().await;
            Ok(())
        }
        Some("--check") => Ok(()),
        _ => Err(err("Usage: hype-status temperature|hotspot|warp-indicator")),
    }
}

fn main() {
    if let Err(error) = glib::MainContext::default().block_on(run()) {
        eprintln!("hype-status: {error}");
        std::process::exit(1);
    }
}
