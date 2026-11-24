use hype_desktop::{
    Result, caffeine, controls, display, drives, err, helpers, network, runtime, session, wallpaper,
};
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let result = gtk::glib::MainContext::default().block_on(run(&args));
    match result {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("hype-control: {error}");
            std::process::exit(1);
        }
    }
}
async fn run(args: &[OsString]) -> Result<i32> {
    let legacy = Path::new(args.first().map(OsString::as_os_str).unwrap_or_default())
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .strip_suffix(".sh")
        .map(|s| s.strip_suffix("-control").unwrap_or(s));
    let (command, arguments) = if let Some(command) = legacy {
        (command, args.get(1..).unwrap_or_default())
    } else {
        (
            args.get(1)
                .map(|command| command.to_str().ok_or_else(|| err("Invalid command name")))
                .transpose()?
                .unwrap_or_default(),
            args.get(2..).unwrap_or_default(),
        )
    };
    if matches!(
        command,
        "fullscreen-toggle"
            | "menu-toggle"
            | "password-prompt"
            | "power-menu"
            | "start-flow"
            | "start-idle"
            | "start-tray-applets"
            | "start-waybar"
            | "stealth-indicator"
            | "warp-indicator"
            | "app-launcher"
            | "stop-waybar"
            | "launch-default"
    ) {
        return helpers::run(command, arguments).await;
    }
    let arguments = arguments
        .iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| err("Invalid command argument"))
        })
        .collect::<Result<Vec<_>>>()?;
    run_desktop(command, &arguments).await?;
    Ok(0)
}

async fn run_desktop(command: &str, arguments: &[String]) -> Result<()> {
    match command {
        "temperature" | "brightness" | "volume" | "mic" | "power-profile"
        | "auto-power-profile" => controls::run(command, arguments).await,
        "niri-outputs" | "display-scale" | "monitor-refresh" | "opacity-toggle" => {
            display::run(command, arguments).await
        }
        "clipboard" => hype_desktop::clipboard::run(arguments).await,
        "emoji" => hype_desktop::emoji::run(arguments).await,
        "media" => hype_desktop::media::run(arguments).await,
        "wallpaper" => wallpaper::run(command, arguments).await,
        "monitors" | "prepare-sleep" | "suspend-now" | "lock-dim" | "copilot-shortcut"
        | "copilot-display" | "lock" | "lock-screen" => session::run(command, arguments).await,
        "caffeine" => caffeine::run(command, arguments).await,
        "hotspot" => network::run(command, arguments).await,
        "stealth" => hype_desktop::stealth_client::run(arguments).await,
        "open-drives" => drives::run(command, arguments).await,
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
                "Usage: hype-control <command> [arguments]\nCommands: monitors clipboard emoji app-launcher stop-waybar launch-default media temperature brightness volume mic power-profile auto-power-profile caffeine display-scale monitor-refresh opacity-toggle hotspot open-drives copilot-shortcut copilot-display prepare-sleep suspend-now lock-dim lock wallpaper runtime-dir stealth fullscreen-toggle menu-toggle password-prompt power-menu start-flow start-idle start-tray-applets start-waybar stealth-indicator warp-indicator"
            );
            Ok(())
        }
        _ => Err(err(format!("Unknown desktop command: {command}"))),
    }
}
