use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::domain::model::SlackError;

/// Save a downloaded file in the user's Downloads folder without replacing
/// anything already there: `name.stl`, then `name (1).stl`, and so on.
pub fn save(name: &str, bytes: &[u8]) -> Result<PathBuf, SlackError> {
    let directory = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .ok_or_else(|| SlackError("Could not find a Downloads folder.".into()))?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        SlackError(format!("Could not create {}: {error}", directory.display()))
    })?;
    save_in(&directory, name, bytes)
}

fn save_in(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, SlackError> {
    let name = safe_name(name);
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name.as_str(), String::new()),
    };
    for attempt in 0..MAX_ATTEMPTS {
        let candidate = if attempt == 0 {
            directory.join(&name)
        } else {
            directory.join(format!("{stem} ({attempt}){extension}"))
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                return file
                    .write_all(bytes)
                    .map(|()| candidate.clone())
                    .map_err(|error| {
                        let _ = std::fs::remove_file(&candidate);
                        SlackError(format!("Could not save {}: {error}", candidate.display()))
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(SlackError(format!(
                    "Could not save {}: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(SlackError(format!(
        "Too many files named {name} in Downloads."
    )))
}

/// The folder cached videos live in. Does not create it.
fn video_dir() -> Result<PathBuf, SlackError> {
    Ok(dirs::cache_dir()
        .ok_or_else(|| SlackError("Could not find a cache folder.".into()))?
        .join("better-slack")
        .join("videos"))
}

/// The file name a cached video gets: the key stripped to alphanumerics so it
/// cannot escape the folder, joined to a sanitised version of the name.
fn video_file_name(file_key: &str, name: &str) -> String {
    let key: String = file_key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(40)
        .collect();
    format!("{key}-{}", safe_name(name))
}

/// Where a video is kept for the system player, under the user cache folder.
pub fn video_path(file_key: &str, name: &str) -> Result<PathBuf, SlackError> {
    let directory = video_dir()?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        SlackError(format!("Could not create {}: {error}", directory.display()))
    })?;
    Ok(directory.join(video_file_name(file_key, name)))
}

/// Keep a downloaded video for replays, dropping videos unused for a week.
pub fn keep_video(path: &Path, bytes: &[u8]) -> Result<(), SlackError> {
    if let Some(directory) = path.parent()
        && let Ok(entries) = std::fs::read_dir(directory)
    {
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified.elapsed().unwrap_or_default() > VIDEO_CACHE_AGE);
            if old {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, path))
        .map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            SlackError(format!("Could not save {}: {error}", path.display()))
        })
}

/// Open a file in the user's default app: xdg-open on Linux, open on macOS.
pub fn open_with_default_app(path: &Path) -> Result<(), SlackError> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(opener)
        .arg(path)
        .spawn()
        .map_err(|error| SlackError(format!("Could not run {opener}: {error}")))?;
    // The opener fails fast without a matching app, but may stay running
    // while the player is open; only an early failure is an error.
    let started = std::time::Instant::now();
    while started.elapsed() < OPENER_GRACE {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(SlackError(format!(
                    "No app is set up to play {}. Install a video player such as mpv.",
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("this file")
                )));
            }
            Ok(Some(_)) => return Ok(()),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(error) => return Err(SlackError(error.to_string())),
        }
    }
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Slack file names are untrusted: keep only the last path component.
fn safe_name(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.');
    if name.is_empty() {
        "download".into()
    } else {
        name.into()
    }
}

const MAX_ATTEMPTS: usize = 1000;
const VIDEO_CACHE_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
const OPENER_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_videos_get_safe_unique_names() {
        assert_eq!(
            video_file_name("F1/../x", "../../Rendered video.mp4"),
            "F1x-Rendered video.mp4"
        );
    }

    #[test]
    fn cached_videos_live_under_the_user_cache_folder() {
        assert!(video_dir().unwrap().ends_with("better-slack/videos"));
    }

    #[test]
    fn never_overwrites_and_strips_paths() {
        let directory = std::env::temp_dir().join(format!("downloads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let first = save_in(&directory, "cup v1.stl", b"one").unwrap();
        let second = save_in(&directory, "cup v1.stl", b"two").unwrap();
        assert_eq!(first, directory.join("cup v1.stl"));
        assert_eq!(second, directory.join("cup v1 (1).stl"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        let escaped = save_in(&directory, "../../.bashrc", b"x").unwrap();
        assert_eq!(escaped, directory.join("bashrc"));
        assert_eq!(
            save_in(&directory, "..", b"x").unwrap(),
            directory.join("download")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
