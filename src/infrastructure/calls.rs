//! The ringtone for incoming huddle calls.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A ringtone played through the system's sound player, repeated until
/// stopped.
#[derive(Default)]
pub struct Ringtone {
    player: Option<Child>,
    last_ring: Option<Instant>,
}

impl Ringtone {
    /// Ring again when the previous ring has had its pause; call regularly.
    pub fn tick(&mut self) {
        if self
            .last_ring
            .is_some_and(|last| last.elapsed() < RING_INTERVAL)
        {
            return;
        }
        self.last_ring = Some(Instant::now());
        self.kill();
        self.player = ring_file().and_then(|path| play(&path));
    }

    pub fn stop(&mut self) {
        self.kill();
        self.last_ring = None;
    }

    fn kill(&mut self) {
        if let Some(mut player) = self.player.take() {
            let _ = player.kill();
            let _ = player.wait();
        }
    }
}

impl Drop for Ringtone {
    fn drop(&mut self) {
        self.kill();
    }
}

const RING_INTERVAL: Duration = Duration::from_millis(2600);

fn play(path: &std::path::Path) -> Option<Child> {
    if cfg!(test) {
        return None;
    }
    let players: &[&str] = if cfg!(target_os = "macos") {
        &["afplay"]
    } else {
        &["pw-play", "paplay", "aplay"]
    };
    players.iter().find_map(|player| {
        Command::new(player)
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    })
}

/// The ring sound, written once to the cache directory.
fn ring_file() -> Option<std::path::PathBuf> {
    static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = dirs::cache_dir()?.join("better-slack").join("ringtone.wav");
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, ring_wav()).ok()?;
        Some(path)
    })
    .clone()
}

/// Two soft rising chimes, as 16-bit mono WAV.
fn ring_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let mut samples: Vec<i16> = Vec::new();
    for (frequency, seconds) in [(659.25, 0.32), (0.0, 0.08), (987.77, 0.42)] {
        let count = (seconds * RATE as f64) as usize;
        for index in 0..count {
            let t = index as f64 / RATE as f64;
            // Quick attack, gentle decay, so the tone never clicks.
            let envelope = (t / 0.012).min(1.0) * (-(t / seconds) * 3.0).exp();
            let tone = (2.0 * std::f64::consts::PI * frequency * t).sin()
                + 0.25 * (4.0 * std::f64::consts::PI * frequency * t).sin();
            samples.push((tone * envelope * 0.32 * i16::MAX as f64) as i16);
        }
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ringtone_is_a_well_formed_wav() {
        let wav = ring_wav();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        assert!(data_len > 22_050, "most of a second of sound");
    }
}
