use std::path::PathBuf;

use eframe::egui;

use crate::domain::model::SlackError;

#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
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

#[cfg(target_os = "linux")]
pub fn read_attachment() -> Result<Option<ClipboardAttachment>, SlackError> {
    use crate::domain::uploads::MAX_BATCH_BYTES;

    let Ok(mut board) = arboard::Clipboard::new() else {
        return Ok(None);
    };
    if let Ok(paths) = board.get().file_list()
        && !paths.is_empty()
    {
        return Ok(Some(ClipboardAttachment::Files(paths)));
    }
    // Some file managers only publish copied files as plain-text file URIs.
    if let Some(paths) = board.get_text().ok().and_then(|text| file_uri_paths(&text)) {
        return Ok(Some(ClipboardAttachment::Files(paths)));
    }
    let Ok(image) = board.get_image() else {
        return Ok(None);
    };
    let pixels = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.into_owned(),
    )
    .ok_or_else(|| SlackError("Could not read the copied image.".into()))?;
    let mut png = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| SlackError(format!("Could not encode the copied image: {error}")))?;
    let png = png.into_inner();
    if png.len() > MAX_BATCH_BYTES {
        return Err(SlackError(
            "Clipboard image exceeds the 100 MB attachment limit.".into(),
        ));
    }
    Ok(Some(ClipboardAttachment::Image(png)))
}

#[cfg(target_os = "linux")]
fn file_uri_paths(text: &str) -> Option<Vec<PathBuf>> {
    let paths = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let path = PathBuf::from(percent_decode(line.strip_prefix("file://")?)?);
            path.exists().then_some(path)
        })
        .collect::<Option<Vec<_>>>()?;
    (!paths.is_empty()).then_some(paths)
}

#[cfg(target_os = "linux")]
fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
    pub fn take(&mut self, _events: &[egui::Event]) -> bool {
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

// egui-winit swallows the Ctrl+V press and, on Wayland, emits nothing for
// image-only clipboards. The key release still arrives, so a V release without
// a reported press marks the shortcut. Text pastes are handled on the press.
#[cfg(target_os = "linux")]
#[derive(Default)]
pub struct PasteShortcut {
    v_down: bool,
    pasted: bool,
}

#[cfg(target_os = "linux")]
impl PasteShortcut {
    pub fn take(&mut self, events: &[egui::Event]) -> bool {
        let mut shortcut = false;
        for event in events {
            match event {
                egui::Event::Paste(_) => self.pasted = true,
                egui::Event::Key {
                    key: egui::Key::V,
                    pressed,
                    repeat: false,
                    ..
                } => {
                    if *pressed {
                        self.v_down = true;
                    } else {
                        shortcut = !self.v_down && !self.pasted;
                        self.v_down = false;
                        self.pasted = false;
                    }
                }
                _ => {}
            }
        }
        shortcut
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[derive(Default)]
pub struct PasteShortcut;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl PasteShortcut {
    pub fn take(&mut self, _events: &[egui::Event]) -> bool {
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    fn key_v(pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::V,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }
    }

    #[test]
    fn detects_swallowed_paste_shortcut_once() {
        let mut shortcut = PasteShortcut::default();
        assert!(shortcut.take(&[key_v(false)]));
        assert!(!shortcut.take(&[key_v(true)]));
        assert!(!shortcut.take(&[key_v(false)]));
        assert!(!shortcut.take(&[egui::Event::Paste("text".into())]));
        assert!(!shortcut.take(&[key_v(false)]));
    }

    #[test]
    fn reads_copied_file_uris() {
        let path = std::env::temp_dir().join(format!("clip board {}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, "fixture").unwrap();
        let uri = format!("file://{}", path.to_str().unwrap().replace(' ', "%20"));
        assert_eq!(
            file_uri_paths(&format!("{uri}\r\n")),
            Some(vec![path.clone()])
        );
        assert_eq!(file_uri_paths(&format!("{uri}\nhello")), None);
        assert_eq!(file_uri_paths("https://example.test/file"), None);
        assert_eq!(file_uri_paths("file:///missing/file.png"), None);
        std::fs::remove_file(path).unwrap();
    }
}
