//! The microphone and speakers: 48 kHz mono in 20 ms frames, which is what
//! the call's Opus stream carries, converted to whatever the devices use.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};

pub const RATE: u32 = 48_000;
/// Samples in one 20 ms frame.
pub const FRAME: usize = 960;
/// Playback drops the oldest sound beyond this, so the call never lags far behind.
const MAX_QUEUED: usize = RATE as usize / 4;

/// Sound waiting to be played, at [`RATE`].
#[derive(Clone, Default)]
pub struct Playback(Arc<Mutex<VecDeque<f32>>>);

impl Playback {
    pub fn push(&self, samples: &[f32]) {
        let Ok(mut queue) = self.0.lock() else {
            return;
        };
        queue.extend(samples);
        let excess = queue.len().saturating_sub(MAX_QUEUED);
        queue.drain(..excess);
    }

    #[cfg(test)]
    fn queued(&self) -> usize {
        self.0.lock().map(|queue| queue.len()).unwrap_or_default()
    }

    fn pop(&self, out: &mut [f32]) {
        let Ok(mut queue) = self.0.lock() else {
            out.fill(0.0);
            return;
        };
        for sample in out {
            *sample = queue.pop_front().unwrap_or(0.0);
        }
    }
}

/// Open audio devices; sound stops when this is dropped.
pub struct Devices {
    _input: Stream,
    _output: Stream,
}

/// Start the default microphone and speakers. The microphone's 20 ms frames
/// arrive on the returned receiver; what is pushed to `playback` is played.
pub fn open(playback: Playback) -> Result<(Devices, Receiver<Vec<f32>>), String> {
    let host = cpal::default_host();
    let input = host
        .default_input_device()
        .ok_or("No microphone was found")?;
    let output = host
        .default_output_device()
        .ok_or("No speakers or headphones were found")?;
    let (frames, received) = sync_channel(50);
    let input_stream = open_input(&input, frames)?;
    let output_stream = open_output(&output, playback)?;
    input_stream.play().map_err(|error| error.to_string())?;
    output_stream.play().map_err(|error| error.to_string())?;
    Ok((
        Devices {
            _input: input_stream,
            _output: output_stream,
        },
        received,
    ))
}

fn open_input(device: &cpal::Device, frames: SyncSender<Vec<f32>>) -> Result<Stream, String> {
    let supported = device
        .default_input_config()
        .map_err(|error| format!("The microphone cannot be opened: {error}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    match format {
        SampleFormat::F32 => input_stream::<f32>(device, config, frames),
        SampleFormat::I16 => input_stream::<i16>(device, config, frames),
        SampleFormat::I32 => input_stream::<i32>(device, config, frames),
        SampleFormat::U16 => input_stream::<u16>(device, config, frames),
        other => Err(format!(
            "The microphone's {other} samples are not supported"
        )),
    }
}

fn input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    frames: SyncSender<Vec<f32>>,
) -> Result<Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let mut resampler = Resampler::new(config.sample_rate, RATE);
    let mut mono = Vec::new();
    let mut resampled = Vec::new();
    let mut frame = Vec::with_capacity(FRAME);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                mono.clear();
                mono.extend(data.chunks(channels).map(|samples| {
                    samples
                        .iter()
                        .map(|&sample| f32::from_sample(sample))
                        .sum::<f32>()
                        / channels as f32
                }));
                resampled.clear();
                resampler.process(&mono, &mut resampled);
                for &sample in &resampled {
                    frame.push(sample);
                    if frame.len() == FRAME {
                        let full = std::mem::replace(&mut frame, Vec::with_capacity(FRAME));
                        // A stalled sender loses sound rather than the call.
                        if let Err(TrySendError::Disconnected(_)) = frames.try_send(full) {
                            return;
                        }
                    }
                }
            },
            |error| eprintln!("Microphone error: {error}"),
            None,
        )
        .map_err(|error| format!("The microphone cannot be opened: {error}"))
}

fn open_output(device: &cpal::Device, playback: Playback) -> Result<Stream, String> {
    let supported = device
        .default_output_config()
        .map_err(|error| format!("The speakers cannot be opened: {error}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    match format {
        SampleFormat::F32 => output_stream::<f32>(device, config, playback),
        SampleFormat::I16 => output_stream::<i16>(device, config, playback),
        SampleFormat::I32 => output_stream::<i32>(device, config, playback),
        SampleFormat::U16 => output_stream::<u16>(device, config, playback),
        other => Err(format!("The speakers' {other} samples are not supported")),
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    playback: Playback,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let mut resampler = Resampler::new(RATE, config.sample_rate);
    let mut source = Vec::new();
    let mut resampled: VecDeque<f32> = VecDeque::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let wanted = data.len() / channels;
                while resampled.len() < wanted {
                    // Enough 48 kHz sound for what is missing, plus one sample
                    // so rounding never leaves the loop short.
                    let need = ((wanted - resampled.len()) as f64 / resampler.step).ceil() as usize;
                    source.resize(need.max(1), 0.0);
                    playback.pop(&mut source);
                    let mut out = Vec::new();
                    resampler.process(&source, &mut out);
                    resampled.extend(out);
                }
                for frame in data.chunks_mut(channels) {
                    let sample = T::from_sample(resampled.pop_front().unwrap_or(0.0));
                    frame.fill(sample);
                }
            },
            |error| eprintln!("Speaker error: {error}"),
            None,
        )
        .map_err(|error| format!("The speakers cannot be opened: {error}"))
}

/// Linear-interpolation sample rate conversion for a continuous stream.
struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Where the next output sample falls, counting the previous call's last
    /// input sample as index 0.
    position: f64,
    last: f32,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            step: from as f64 / to.max(1) as f64,
            position: 1.0,
            last: 0.0,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.step == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        let at = |index: usize| {
            if index == 0 {
                self.last
            } else {
                input[index - 1]
            }
        };
        while self.position <= input.len() as f64 {
            let index = self.position.floor() as usize;
            let fraction = (self.position - index as f64) as f32;
            let a = at(index);
            let b = if index < input.len() {
                at(index + 1)
            } else {
                a
            };
            out.push(a + (b - a) * fraction);
            self.position += self.step;
        }
        self.position -= input.len() as f64;
        self.last = input[input.len() - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_the_rate_across_calls() {
        let mut down = Resampler::new(48_000, 16_000);
        let mut up = Resampler::new(16_000, 48_000);
        let mut low = Vec::new();
        let mut high = Vec::new();
        for _ in 0..50 {
            let mut chunk = Vec::new();
            down.process(&[0.5; 960], &mut chunk);
            up.process(&chunk, &mut high);
            low.extend(chunk);
        }
        assert!((low.len() as i64 - 16_000).abs() <= 1, "{}", low.len());
        assert!((high.len() as i64 - 48_000).abs() <= 3, "{}", high.len());
        assert!(
            high[100..]
                .iter()
                .all(|&sample| (sample - 0.5).abs() < 1e-6)
        );
    }

    #[test]
    fn playback_never_falls_far_behind() {
        let playback = Playback::default();
        for _ in 0..100 {
            playback.push(&[0.1; FRAME]);
        }
        assert_eq!(playback.queued(), MAX_QUEUED);
        let mut out = [1.0; 4];
        Playback::default().pop(&mut out);
        assert_eq!(out, [0.0; 4]);
    }
}
