use hype_desktop::{
    Result, caffeine, controls, display, drives, err, network, runtime, session, wallpaper,
};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = gtk::glib::MainContext::default().block_on(run(&args));
    if let Err(error) = result {
        eprintln!("hype-control: {error}");
        std::process::exit(1);
    }
}
async fn run(args: &[String]) -> Result<()> {
    let legacy = std::path::Path::new(args.first().map(String::as_str).unwrap_or_default())
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .strip_suffix(".sh")
        .map(|s| s.strip_suffix("-control").unwrap_or(s).to_owned());
    let (command, arguments) = if let Some(command) = legacy {
        (command, args.get(1..).unwrap_or_default())
    } else {
        (
            args.get(1).cloned().unwrap_or_default(),
            args.get(2..).unwrap_or_default(),
        )
    };
    match command.as_str() {
        "temperature" | "brightness" | "volume" | "mic" | "power-profile"
        | "auto-power-profile" => controls::run(&command, arguments).await,
        "niri-outputs" | "display-scale" | "monitor-refresh" | "opacity-toggle" => {
            display::run(&command, arguments).await
        }
        "wallpaper" => wallpaper::run(&command, arguments).await,
        "prepare-sleep" | "suspend-now" | "lock-dim" | "copilot-shortcut" | "copilot-display"
        | "lock" | "lock-screen" => session::run(&command, arguments).await,
        "caffeine" => caffeine::run(&command, arguments).await,
        "hotspot" => network::run(&command, arguments).await,
        "stealth" => hype_desktop::stealth_client::run(arguments).await,
        "open-drives" => drives::run(&command, arguments).await,
        "runtime-dir" => {
            println!("{}", runtime::dir()?.display());
            Ok(())
        }
        "--check" => {
            println!("Native desktop controls, D-Bus and IPC interfaces ready");
            Ok(())
        }
        "--help" | "help" | "" => {
            println!(
                "Usage: hype-control <command> [arguments]\nCommands: temperature brightness volume mic power-profile auto-power-profile caffeine display-scale monitor-refresh opacity-toggle hotspot open-drives copilot-shortcut copilot-display prepare-sleep suspend-now lock-dim lock wallpaper runtime-dir stealth"
            );
            Ok(())
        }
        _ => Err(err(format!("Unknown desktop command: {command}"))),
    }
}
