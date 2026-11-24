use crate::{Result, dialog, err, notify};
use gtk::{gio, prelude::*};

fn applications(category: Option<&str>) -> Vec<gio::AppInfo> {
    let mut apps: Vec<_> = gio::AppInfo::all()
        .into_iter()
        .filter(|app| app.should_show())
        .filter(|app| {
            category.is_none_or(|category| {
                app.downcast_ref::<gio_unix::DesktopAppInfo>()
                    .and_then(|app| app.categories())
                    .is_some_and(|categories| categories.split(';').any(|item| item == category))
            })
        })
        .collect();
    apps.sort_by_cached_key(|app| app.display_name().to_lowercase());
    apps
}

async fn choose(title: &str, apps: &[gio::AppInfo]) -> Result<()> {
    let Some(index) = dialog::choose_apps(title, apps).await? else {
        return Ok(());
    };
    apps[index].launch(&[], None::<&gio::AppLaunchContext>)?;
    Ok(())
}

pub async fn run() -> Result<()> {
    choose("Applications", &applications(None)).await
}

pub async fn launch_default(kind: &str) -> Result<()> {
    if kind == "color-picker" {
        return match std::process::Command::new("hyprpicker").arg("-a").spawn() {
            Ok(_) => Ok(()),
            Err(error) => {
                let message =
                    format!("Install the optional hyprpicker application to pick colors: {error}");
                let _ = notify::send(
                    0,
                    "Color picker unavailable",
                    &message,
                    None,
                    "applications",
                )
                .await;
                Err(err(message))
            }
        };
    }
    let default = match kind {
        "browser" => gio::AppInfo::default_for_uri_scheme("https")
            .or_else(|| gio::AppInfo::default_for_uri_scheme("http")),
        "editor" => gio::AppInfo::default_for_type("text/plain", false),
        "monitor" | "audio" => None,
        _ => {
            return Err(err(
                "Usage: launch-default browser|editor|monitor|audio|color-picker",
            ));
        }
    };
    if let Some(app) = default {
        app.launch(&[], None::<&gio::AppLaunchContext>)?;
        return Ok(());
    }
    let (title, category) = match kind {
        "browser" => ("Web browser", "WebBrowser"),
        "editor" => ("Text editor", "TextEditor"),
        "monitor" => ("System monitor", "Monitor"),
        _ => ("Audio controls", "Mixer"),
    };
    let apps = applications(Some(category));
    if apps.len() == 1 {
        apps[0].launch(&[], None::<&gio::AppLaunchContext>)?;
    } else if !apps.is_empty() {
        choose(title, &apps).await?;
    } else {
        let message = format!(
            "Install an application for {title} or choose one in the application launcher."
        );
        let _ = notify::send(
            0,
            "No application configured",
            &message,
            None,
            "applications",
        )
        .await;
        return Err(err(message));
    }
    Ok(())
}
