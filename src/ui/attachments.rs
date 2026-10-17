use std::io::{Cursor, Read};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use crate::domain::model::SlackError;
use crate::domain::uploads::{MAX_BATCH_BYTES, MAX_FILES, UploadFile};
use crate::infrastructure::clipboard::ClipboardAttachment;

struct PreparedFile {
    file: UploadFile,
    thumbnail: Option<egui::ColorImage>,
}

struct DraftFile {
    file: UploadFile,
    texture: Option<egui::TextureHandle>,
}

#[derive(Default)]
pub struct Attachments {
    files: Vec<DraftFile>,
    preparing: Option<Receiver<Result<Vec<PreparedFile>, SlackError>>>,
    pub error: Option<String>,
}

impl Attachments {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
    pub fn is_preparing(&self) -> bool {
        self.preparing.is_some()
    }

    pub fn paste(&mut self, content: ClipboardAttachment, ctx: &egui::Context) {
        if self.is_preparing() {
            self.error = Some("Wait for the current attachments to finish preparing.".into());
            return;
        }
        let count = self.files.len();
        let bytes = self.files.iter().map(|file| file.file.bytes.len()).sum();
        let (sender, receiver) = channel();
        self.preparing = Some(receiver);
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = prepare(content, count, bytes);
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    pub fn take(&mut self) -> Vec<UploadFile> {
        self.error = None;
        std::mem::take(&mut self.files)
            .into_iter()
            .map(|file| file.file)
            .collect()
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.preparing else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err(SlackError(
                "Could not prepare attachments. Please paste again.".into(),
            )),
        };
        self.preparing = None;
        match result {
            Ok(files) => self
                .files
                .extend(files.into_iter().map(|prepared| DraftFile {
                    texture: prepared.thumbnail.map(|image| {
                        ctx.load_texture(
                            format!("draft-{}", uuid::Uuid::new_v4()),
                            image,
                            egui::TextureOptions::LINEAR,
                        )
                    }),
                    file: prepared.file,
                })),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        if self.files.is_empty() && !self.is_preparing() && self.error.is_none() {
            return;
        }
        ui.add_space(8.0);
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt("draft-attachments")
            .max_height(168.0)
            .show(ui, |ui| {
                for (index, file) in self.files.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if let Some(texture) = &file.texture {
                            ui.add(
                                egui::Image::new(texture).fit_to_exact_size(egui::vec2(40.0, 40.0)),
                            );
                        } else {
                            ui.add_sized(
                                [40.0, 40.0],
                                egui::Label::new(egui::RichText::new("File").small().weak()),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove attachment")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.label(
                                egui::RichText::new(file_size(file.file.bytes.len()))
                                    .small()
                                    .weak(),
                            );
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(egui::Label::new(&file.file.name).truncate());
                                },
                            );
                        });
                    });
                }
            });
        if let Some(index) = remove {
            self.files.remove(index);
        }
        if self.is_preparing() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Preparing attachments…").small().weak());
                if ui.small_button("Cancel").clicked() {
                    self.preparing = None;
                }
            });
        }
        if let Some(error) = self.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(super::theme::UNREAD_DOT, error);
                if ui.small_button("Dismiss").clicked() {
                    self.error = None;
                }
            });
        }
    }
}

fn prepare(
    content: ClipboardAttachment,
    existing_count: usize,
    existing_bytes: usize,
) -> Result<Vec<PreparedFile>, SlackError> {
    let mut files = Vec::new();
    let mut bytes = existing_bytes;
    match content {
        ClipboardAttachment::Files(paths) => {
            if existing_count + paths.len() > MAX_FILES {
                return Err(SlackError("Attach up to 10 files per message.".into()));
            }
            for path in paths {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file")
                    .to_owned();
                let file = std::fs::File::open(&path).map_err(|_| {
                    SlackError(format!("Could not read {name}. Copy the file again."))
                })?;
                let metadata = file
                    .metadata()
                    .map_err(|_| SlackError(format!("Could not read {name}.")))?;
                if !metadata.is_file() {
                    return Err(SlackError(
                        "Folders cannot be attached. Compress the folder first.".into(),
                    ));
                }
                check_size(bytes, metadata.len())?;
                let mut data = Vec::with_capacity(metadata.len() as usize);
                file.take((MAX_BATCH_BYTES - bytes) as u64 + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| SlackError(format!("Could not read {name}.")))?;
                check_size(bytes, data.len() as u64)?;
                bytes += data.len();
                files.push(prepared_file(name, data));
            }
        }
        ClipboardAttachment::Image(data) => {
            if existing_count >= MAX_FILES {
                return Err(SlackError("Attach up to 10 files per message.".into()));
            }
            check_size(bytes, data.len() as u64)?;
            let format = image::guess_format(&data)
                .map_err(|_| SlackError("Unsupported clipboard image.".into()))?;
            // Screenshots often arrive as TIFF. Normalize them to PNG for Slack.
            let (data, extension) = match format {
                image::ImageFormat::Png => (data, "png"),
                image::ImageFormat::Jpeg => (data, "jpg"),
                _ => {
                    let image = decode(&data).ok_or_else(|| {
                        SlackError("Could not decode the clipboard image.".into())
                    })?;
                    let mut png = Cursor::new(Vec::new());
                    image
                        .write_to(&mut png, image::ImageFormat::Png)
                        .map_err(|_| SlackError("Could not prepare the clipboard image.".into()))?;
                    (png.into_inner(), "png")
                }
            };
            check_size(bytes, data.len() as u64)?;
            let name = format!(
                "Image-{}.{}",
                chrono::Local::now().format("%Y%m%d-%H%M%S"),
                extension
            );
            files.push(prepared_file(name, data));
        }
    }
    Ok(files)
}

fn check_size(existing: usize, added: u64) -> Result<(), SlackError> {
    if added == 0 {
        return Err(SlackError("Empty files cannot be uploaded.".into()));
    }
    if added > MAX_BATCH_BYTES.saturating_sub(existing) as u64 {
        return Err(SlackError(
            "Attachments must total 100 MB or less per message.".into(),
        ));
    }
    Ok(())
}

fn decode(data: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

fn prepared_file(name: String, data: Vec<u8>) -> PreparedFile {
    let thumbnail = decode(&data).map(|image| {
        let rgba = image.thumbnail(160, 160).into_rgba8();
        egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            &rgba,
        )
    });
    PreparedFile {
        file: UploadFile {
            name,
            bytes: data.into(),
        },
        thumbnail,
    }
}

fn file_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_bytes(format: image::ImageFormat) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            12,
            8,
            image::Rgba([45, 120, 170, 255]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn clipboard_png_is_preserved_and_tiff_is_converted_with_a_thumbnail() {
        let png = image_bytes(image::ImageFormat::Png);
        let files = prepare(ClipboardAttachment::Image(png.clone()), 0, 0).unwrap();
        assert_eq!(&*files[0].file.bytes, &png);
        assert!(files[0].thumbnail.is_some());
        let files = prepare(
            ClipboardAttachment::Image(image_bytes(image::ImageFormat::Tiff)),
            0,
            0,
        )
        .unwrap();
        assert_eq!(
            image::guess_format(&files[0].file.bytes).unwrap(),
            image::ImageFormat::Png
        );
        assert!(files[0].file.name.ends_with(".png"));
        let image = image::load_from_memory(&files[0].file.bytes).unwrap();
        assert_eq!((image.width(), image.height()), (12, 8));
    }

    #[test]
    fn finder_files_are_snapshots_and_folders_and_excess_size_are_rejected() {
        let dir =
            std::env::temp_dir().join(format!("better-slack-attachments-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let text = dir.join("notes with spaces.txt");
        let image = dir.join("picture.png");
        std::fs::write(&text, "original text").unwrap();
        std::fs::write(&image, image_bytes(image::ImageFormat::Png)).unwrap();
        let files = prepare(
            ClipboardAttachment::Files(vec![text.clone(), image.clone()]),
            0,
            0,
        )
        .unwrap();
        std::fs::write(&text, "changed text").unwrap();
        assert_eq!(&*files[0].file.bytes, b"original text");
        assert_eq!(files[0].file.name, "notes with spaces.txt");
        assert!(files[0].thumbnail.is_none());
        assert!(files[1].thumbnail.is_some());
        assert!(prepare(ClipboardAttachment::Files(vec![dir.clone()]), 0, 0).is_err());
        assert!(
            prepare(
                ClipboardAttachment::Files(vec![image.clone()]),
                MAX_FILES,
                0
            )
            .is_err()
        );
        assert!(prepare(ClipboardAttachment::Files(vec![image]), 0, MAX_BATCH_BYTES).is_err());
        assert!(check_size(0, 0).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn attachment_only_drafts_submit_and_keep_composer_focus() {
        use super::super::composer::{ComposeTarget, Draft, composer};
        let ctx = egui::Context::default();
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: Some("10.0".into()),
        };
        let mut draft = Draft::default();
        draft.attachments.files.push(DraftFile {
            file: UploadFile {
                name: "notes.txt".into(),
                bytes: Vec::from(b"notes").into(),
            },
            texture: None,
        });
        ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
        let mut submitted = false;
        for events in [
            vec![],
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320.0, 500.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    submitted = composer(ui, &target, &mut draft, "Reply", None);
                },
            );
            output.textures_delta.clear();
        }
        assert!(submitted);
        assert!(draft.text.is_empty());
        assert!(ctx.memory(|memory| memory.has_focus(target.editor_id())));
        assert_eq!(draft.attachments.take()[0].name, "notes.txt");
        assert!(draft.attachments.is_empty());
    }
}
