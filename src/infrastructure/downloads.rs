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

#[cfg(test)]
mod tests {
    use super::*;

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
