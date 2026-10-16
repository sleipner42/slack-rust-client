//! Desktop notifications: D-Bus on Linux, Notification Center on macOS.

/// Show a notification without blocking. `on_click` runs if the user clicks it
/// (Linux; on macOS a click brings the app forward).
pub fn show(title: &str, body: &str, on_click: Option<Box<dyn FnOnce() + Send>>) {
    if cfg!(test) {
        return;
    }
    let (title, body) = (title.to_owned(), body.to_owned());
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body);
        #[cfg(target_os = "macos")]
        set_application();
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            notification.appname(APP_NAME);
            if let Some(icon) = icon_path() {
                notification.icon(&icon.to_string_lossy());
            }
            if on_click.is_some() {
                notification.action("default", "Open");
            }
        }
        let Ok(handle) = notification.show() else {
            return;
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(on_click) = on_click {
            handle.wait_for_action(|action| {
                if action == "default" {
                    on_click();
                }
            });
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = (handle, on_click);
    });
}

/// Post as this app's bundle so the notification carries its name and icon.
#[cfg(target_os = "macos")]
fn set_application() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(id) = objc2_foundation::NSBundle::mainBundle().bundleIdentifier() {
            let _ = notify_rust::set_application(&id.to_string());
        }
    });
}

/// The bundled icon, written once to the cache directory for the notification daemon.
#[cfg(all(unix, not(target_os = "macos")))]
fn icon_path() -> Option<std::path::PathBuf> {
    static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = dirs::cache_dir()?.join("better-slack").join("app-icon.png");
        if !path.exists() {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, include_bytes!("../../assets/app-icon.png")).ok()?;
        }
        Some(path)
    })
    .clone()
}

#[cfg(all(unix, not(target_os = "macos")))]
const APP_NAME: &str = "Better Slack";
