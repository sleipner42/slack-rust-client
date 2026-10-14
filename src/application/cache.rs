use sha1::{Digest, Sha1};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

pub struct DiskCache {
    root: PathBuf,
    last_prune: Mutex<Option<Instant>>,
}

impl DiskCache {
    pub fn for_account(namespace: &str) -> Option<Self> {
        Some(Self::at(
            dirs::cache_dir()?
                .join("better-slack/v1")
                .join(key(namespace)),
        ))
    }

    pub(super) fn at(root: PathBuf) -> Self {
        Self {
            root,
            last_prune: Mutex::new(None),
        }
    }

    fn path(&self, kind: &str, id: &str) -> PathBuf {
        self.root.join(format!("{kind}-{}", key(id)))
    }

    pub fn read(&self, kind: &str, id: &str) -> Option<Vec<u8>> {
        let path = self.path(kind, id);
        let metadata = fs::metadata(&path).ok()?;
        if metadata.len() > MAX_ENTRY || metadata.modified().ok()?.elapsed().ok()? > MAX_AGE {
            return None;
        }
        fs::read(path).ok()
    }

    pub fn write(&self, kind: &str, id: &str, bytes: &[u8]) {
        if bytes.len() as u64 > MAX_ENTRY {
            return;
        }
        let _ = self.try_write(kind, id, bytes);
        self.prune();
    }

    fn try_write(&self, kind: &str, id: &str, bytes: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        let destination = self.path(kind, id);
        let temporary = destination.with_extension(format!(
            "{}-{}-tmp",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            fs::rename(&temporary, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn prune(&self) {
        let Ok(mut last) = self.last_prune.try_lock() else {
            return;
        };
        if last.is_some_and(|time| time.elapsed() < Duration::from_secs(60)) {
            return;
        }
        *last = Some(Instant::now());
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata.is_file().then(|| {
                    (
                        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        metadata.len(),
                        entry.path(),
                    )
                })
            })
            .collect();
        files.sort_by_key(|entry| entry.0);
        let mut total: u64 = files.iter().map(|entry| entry.1).sum();
        for (modified, size, path) in files {
            if total <= MAX_BYTES && modified.elapsed().unwrap_or_default() <= MAX_AGE {
                continue;
            }
            if fs::remove_file(path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

pub fn key(value: &str) -> String {
    format!("{:x}", Sha1::digest(value.as_bytes()))
}
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRY: u64 = 32 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_round_trips_and_isolates_namespaces_and_file_names() {
        let root = std::env::temp_dir().join(format!(
            "better-slack-cache-test-{}-{}",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let cache = DiskCache::at(root.clone());
        assert!(cache.read("image", "https://private/file").is_none());
        cache.write("image", "https://private/file", b"pixels");
        assert_eq!(
            cache.read("image", "https://private/file").unwrap(),
            b"pixels"
        );
        assert!(cache.read("history", "https://private/file").is_none());
        assert!(cache.read("image", "https://private/other").is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
