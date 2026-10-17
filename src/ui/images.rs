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
    results: Receiver<(String, Option<Vec<Frame>>)>,
    ctx: egui::Context,
}

enum ImageState {
    Loading,
    Ready {
        /// One texture for a still image; every frame of an animated GIF.
        frames: Vec<(egui::TextureHandle, Duration)>,
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
                Some(frames) => ImageState::Ready {
                    frames: self.upload(&url, frames),
                    used: Instant::now(),
                },
                None => ImageState::Failed(Instant::now()),
            };
            self.states.insert(url, state);
            self.ctx.request_repaint();
        }
        self.evict();
    }

    fn upload(&self, url: &str, frames: Vec<Frame>) -> Vec<(egui::TextureHandle, Duration)> {
        frames
            .into_iter()
            .enumerate()
            .map(|(index, (image, delay))| {
                let name = if index == 0 {
                    url.to_owned()
                } else {
                    format!("{url}#{index}")
                };
                (
                    self.ctx
                        .load_texture(name, image, egui::TextureOptions::LINEAR),
                    delay,
                )
            })
            .collect()
    }

    /// The image, or for an animation its current frame. Animations repaint
    /// only while something keeps drawing them.
    pub fn image(&mut self, url: &str) -> Option<egui::Image<'static>> {
        match self.states.get_mut(url) {
            Some(ImageState::Ready { frames, used }) => {
                *used = Instant::now();
                let texture = if frames.len() > 1 {
                    let (texture, remaining) = current_frame(frames, self.ctx.input(|i| i.time));
                    self.ctx.request_repaint_after(remaining);
                    texture
                } else {
                    &frames[0].0
                };
                Some(egui::Image::new((texture.id(), texture.size_vec2())))
            }
            Some(ImageState::Loading) => None,
            Some(ImageState::Failed(at)) if at.elapsed() < Duration::from_secs(60) => None,
            _ => {
                // Bundled emoji decode in microseconds; skip the download queue.
                if let Some(frames) = bundled_image(url).and_then(|png| decode_image(png.to_vec()))
                {
                    let frames = self.upload(url, frames);
                    let texture = &frames[0].0;
                    let image = egui::Image::new((texture.id(), texture.size_vec2()));
                    self.states.insert(
                        url.to_string(),
                        ImageState::Ready {
                            frames,
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
                ImageState::Ready { frames, .. } => Some(byte_size(frames)),
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
                ImageState::Ready { frames, used } if used.elapsed() > Duration::from_secs(2) => {
                    Some((*used, url.clone(), byte_size(frames)))
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

type Frame = (egui::ColorImage, Duration);

fn byte_size(frames: &[(egui::TextureHandle, Duration)]) -> usize {
    frames.iter().map(|(texture, _)| texture.byte_size()).sum()
}

/// The frame showing at `time` seconds, and how long until the next one.
fn current_frame(
    frames: &[(egui::TextureHandle, Duration)],
    time: f64,
) -> (&egui::TextureHandle, Duration) {
    let total: Duration = frames.iter().map(|(_, delay)| *delay).sum();
    let mut at = Duration::from_secs_f64(time.max(0.0) % total.as_secs_f64().max(0.001));
    for (texture, delay) in frames {
        if at < *delay {
            return (texture, *delay - at);
        }
        at -= *delay;
    }
    (&frames[0].0, frames[0].1)
}

/// Small animated GIFs (custom emoji, reaction GIFs) keep all their frames;
/// anything else decodes to a single still image.
fn decode_image(bytes: Vec<u8>) -> Option<Vec<Frame>> {
    if bytes.starts_with(b"GIF8")
        && let Some(frames) = decode_animation(&bytes)
    {
        return Some(frames);
    }
    decode_still(bytes).map(|image| vec![(image, Duration::ZERO)])
}

fn decode_animation(bytes: &[u8]) -> Option<Vec<Frame>> {
    use image::AnimationDecoder as _;
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        let buffer = frame.buffer();
        if buffer.width() > MAX_ANIMATION_SIDE
            || buffer.height() > MAX_ANIMATION_SIDE
            || frames.len() == MAX_ANIMATION_FRAMES
        {
            return None;
        }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = Duration::from_millis(u64::from(numerator / denominator.max(1)));
        // Browsers stretch near-zero delays the same way.
        let delay = if delay < MIN_FRAME_DELAY {
            DEFAULT_FRAME_DELAY
        } else {
            delay
        };
        frames.push((
            egui::ColorImage::from_rgba_unmultiplied(
                [buffer.width() as usize, buffer.height() as usize],
                buffer.as_raw(),
            ),
            delay,
        ));
    }
    (frames.len() > 1).then_some(frames)
}

fn decode_still(bytes: Vec<u8>) -> Option<egui::ColorImage> {
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
const MAX_ANIMATION_SIDE: u32 = 512;
const MAX_ANIMATION_FRAMES: usize = 300;
const MIN_FRAME_DELAY: Duration = Duration::from_millis(20);
const DEFAULT_FRAME_DELAY: Duration = Duration::from_millis(100);

#[cfg(test)]
mod animation_tests {
    use super::*;

    fn gif(frames: &[[u8; 4]], delay_ms: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .encode_frames(frames.iter().map(|color| {
                    image::Frame::from_parts(
                        image::RgbaImage::from_pixel(8, 8, image::Rgba(*color)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(delay_ms, 1),
                    )
                }))
                .unwrap();
        }
        bytes
    }

    #[test]
    fn animated_gifs_keep_their_frames_and_cycle_through_them() {
        let frames = decode_image(gif(&[[255, 0, 0, 255], [0, 0, 255, 255]], 200)).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1, Duration::from_millis(200));
        assert_eq!(frames[1].0.pixels[0], egui::Color32::BLUE);

        let ctx = egui::Context::default();
        let textures: Vec<_> = frames
            .into_iter()
            .enumerate()
            .map(|(index, (image, delay))| {
                (
                    ctx.load_texture(format!("f{index}"), image, Default::default()),
                    delay,
                )
            })
            .collect();
        let (shown, next) = current_frame(&textures, 0.05);
        assert_eq!(shown.id(), textures[0].0.id());
        assert_eq!(next, Duration::from_millis(150));
        assert_eq!(current_frame(&textures, 0.25).0.id(), textures[1].0.id());
        assert_eq!(current_frame(&textures, 0.45).0.id(), textures[0].0.id());

        let still = decode_image(gif(&[[255, 0, 0, 255]], 0)).unwrap();
        assert_eq!(still.len(), 1, "a one-frame GIF is a still image");
        let fast = decode_image(gif(&[[1, 1, 1, 255], [2, 2, 2, 255]], 0)).unwrap();
        assert_eq!(fast[0].1, DEFAULT_FRAME_DELAY);
    }
}
