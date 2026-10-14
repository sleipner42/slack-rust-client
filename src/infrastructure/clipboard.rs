use std::path::PathBuf;

use crate::domain::model::SlackError;

pub enum ClipboardAttachment {
    Files(Vec<PathBuf>),
    Image(Vec<u8>),
}

#[cfg(target_os = "macos")]
pub fn read_attachment() -> Result<Option<ClipboardAttachment>, SlackError> {
    use crate::domain::uploads::MAX_BATCH_BYTES;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    let board = NSPasteboard::generalPasteboard();
    let file_type = NSString::from_str("public.file-url");
    let mut paths = Vec::new();
    if let Some(items) = board.pasteboardItems() {
        for item in items {
            if let Some(url) = item.stringForType(&file_type) {
                let path = file_url_path(&url)
                    .ok_or_else(|| SlackError("Could not read the copied file location.".into()))?;
                paths.push(path);
            }
        }
    }
    // Finder publishes both file URLs and an image preview. Keep the actual files.
    if !paths.is_empty() {
        return Ok(Some(ClipboardAttachment::Files(paths)));
    }
    for kind in ["public.png", "public.tiff", "public.jpeg"] {
        if let Some(data) = board.dataForType(&NSString::from_str(kind)) {
            if data.len() > MAX_BATCH_BYTES {
                return Err(SlackError(
                    "Clipboard image exceeds the 100 MB attachment limit.".into(),
                ));
            }
            return Ok(Some(ClipboardAttachment::Image(data.to_vec())));
        }
    }
    Ok(None)
}

#[cfg(target_os = "macos")]
fn file_url_path(value: &objc2_foundation::NSString) -> Option<PathBuf> {
    // Finder may supply a file-reference URL (/.file/id=…), not a filesystem
    // path. Foundation resolves both references and regular percent-encoded URLs.
    let url = objc2_foundation::NSURL::URLWithString(value)?;
    if !url.isFileURL() {
        return None;
    }
    let path = url.filePathURL()?.path()?;
    Some(PathBuf::from(path.to_string()))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2_foundation::{NSString, NSURL};

    #[test]
    fn resolves_finder_reference_urls_and_paths_with_spaces() {
        let path =
            std::env::temp_dir().join(format!("clipboard reference {}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, "fixture").unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
        let reference = url.fileReferenceURL().unwrap();
        let resolved = file_url_path(&reference.absoluteString().unwrap()).unwrap();
        assert_eq!(
            std::fs::canonicalize(resolved).unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );
        assert_eq!(file_url_path(&url.absoluteString().unwrap()).unwrap(), path);
        assert!(file_url_path(&NSString::from_str("https://example.test/file")).is_none());
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_attachment() -> Result<Option<ClipboardAttachment>, SlackError> {
    Ok(None)
}

// egui-winit consumes Cmd+V before emitting raw input, and emits nothing for
// image-only clipboards. Observe the local shortcut without swallowing the event.
// No global keyboard monitor or clipboard polling is used.
#[cfg(target_os = "macos")]
pub struct PasteShortcut {
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    monitor: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
}

#[cfg(target_os = "macos")]
impl Default for PasteShortcut {
    fn default() -> Self {
        use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags as Flags};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let pending = Arc::new(AtomicBool::new(false));
        let signal = pending.clone();
        let block = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
            // SAFETY: AppKit supplies a valid event for the callback's duration.
            let key = unsafe { event.as_ref() };
            let flags = key.modifierFlags();
            if key.keyCode() == 9
                && flags.contains(Flags::Command)
                && !flags.intersects(Flags::Option | Flags::Control | Flags::Shift)
                && !key.isARepeat()
            {
                signal.store(true, Ordering::Release);
            }
            event.as_ptr()
        });
        let monitor = objc2::MainThreadMarker::new().and_then(|_| {
            // SAFETY: The callback returns the original valid event, unchanged.
            unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
            }
        });
        Self { pending, monitor }
    }
}

#[cfg(target_os = "macos")]
impl PasteShortcut {
    pub fn take(&self) -> bool {
        self.pending
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }
}

#[cfg(target_os = "macos")]
impl Drop for PasteShortcut {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            // SAFETY: This token was returned by AppKit's addLocalMonitor method.
            unsafe {
                objc2_app_kit::NSEvent::removeMonitor(monitor);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct PasteShortcut;
#[cfg(not(target_os = "macos"))]
impl PasteShortcut {
    pub fn take(&self) -> bool {
        false
    }
}
