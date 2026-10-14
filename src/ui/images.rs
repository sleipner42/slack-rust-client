use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::application::chat_service::ChatService;
use crate::application::emoji_catalog::bundled_image;
use eframe::egui;

pub struct ImageCache {
    states: HashMap<String, ImageState>,
    requests: Sender<String>,
    results: Receiver<(String, Option<egui::ColorImage>)>,
    ctx: egui::Context,
}

enum ImageState {
    Loading,
    Ready {
        texture: egui::TextureHandle,
        used: Instant,
    },
    Failed(Instant),
}

impl ImageCache {
    pub fn new(service: Arc<ChatService>, ctx: egui::Context) -> Self {
        let (requests, request_receiver) = channel::<String>();
        let (result_sender, results) = channel();
        let shared_receiver = Arc::new(Mutex::new(request_receiver));
        for _ in 0..3 {
            let receiver = Arc::clone(&shared_receiver);
            let sender = result_sender.clone();
            let service = Arc::clone(&service);
            let ctx = ctx.clone();
            thread::spawn(move || {
                loop {
                    let next = receiver.lock().map(|guard| guard.recv());
                    let Ok(Ok(url)) = next else {
                        return;
                    };
                    // Disk reads, network, decompression and downscaling stay off the UI thread.
                    let image = service.download(&url).ok().and_then(decode_image);
                    if sender.send((url, image)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Self {
            states: HashMap::new(),
            requests,
            results,
            ctx,
        }
    }

    pub fn poll(&mut self) {
        // Bound GPU uploads per frame so a batch of downloads cannot stall scrolling.
        for _ in 0..2 {
            let Ok((url, image)) = self.results.try_recv() else {
                break;
            };
            let state = match image {
                Some(image) => ImageState::Ready {
                    texture: self
                        .ctx
                        .load_texture(&url, image, egui::TextureOptions::LINEAR),
                    used: Instant::now(),
                },
                None => ImageState::Failed(Instant::now()),
            };
            self.states.insert(url, state);
            self.ctx.request_repaint();
        }
        self.evict();
    }

    pub fn image(&mut self, url: &str) -> Option<egui::Image<'static>> {
        match self.states.get_mut(url) {
            Some(ImageState::Ready { texture, used }) => {
                *used = Instant::now();
                Some(egui::Image::new((texture.id(), texture.size_vec2())))
            }
            Some(ImageState::Loading) => None,
            Some(ImageState::Failed(at)) if at.elapsed() < Duration::from_secs(60) => None,
            _ => {
                // Bundled emoji decode in microseconds; skip the download queue.
                if let Some(image) = bundled_image(url).and_then(|png| decode_image(png.to_vec())) {
                    let texture = self
                        .ctx
                        .load_texture(url, image, egui::TextureOptions::LINEAR);
                    let image = egui::Image::new((texture.id(), texture.size_vec2()));
                    self.states.insert(
                        url.to_string(),
                        ImageState::Ready {
                            texture,
                            used: Instant::now(),
                        },
                    );
                    return Some(image);
                }
                self.states.insert(url.to_string(), ImageState::Loading);
                let _ = self.requests.send(url.to_string());
                None
            }
        }
    }

    pub fn failed(&self, url: &str) -> bool {
        matches!(self.states.get(url), Some(ImageState::Failed(_)))
    }

    pub fn retry(&mut self, url: &str) {
        if self.failed(url) {
            self.states.remove(url);
        }
    }

    fn evict(&mut self) {
        let mut total: usize = self
            .states
            .values()
            .filter_map(|state| match state {
                ImageState::Ready { texture, .. } => Some(texture.byte_size()),
                _ => None,
            })
            .sum();
        if total <= MEMORY_LIMIT {
            return;
        }
        let mut candidates: Vec<_> = self
            .states
            .iter()
            .filter_map(|(url, state)| match state {
                ImageState::Ready { texture, used } if used.elapsed() > Duration::from_secs(2) => {
                    Some((*used, url.clone(), texture.byte_size()))
                }
                _ => None,
            })
            .collect();
        candidates.sort_by_key(|entry| entry.0);
        for (_, url, bytes) in candidates {
            if total <= MEMORY_LIMIT {
                break;
            }
            self.states.remove(&url);
            total = total.saturating_sub(bytes);
        }
    }
}

fn decode_image(bytes: Vec<u8>) -> Option<egui::ColorImage> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if image.width() > 2560 || image.height() > 2560 {
        image.thumbnail(2560, 2560)
    } else {
        image
    };
    let rgba = image.into_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

const MEMORY_LIMIT: usize = 96 * 1024 * 1024;
