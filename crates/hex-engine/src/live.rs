//! Fixed demo session with audio-thread ownership and preloaded clip media.
use crate::Frame;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

pub const TRACKS: usize = 4;
pub const NAMES: [&str; TRACKS] = ["kick", "pulse", "bass", "texture"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Arrangement,
    Session,
}
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Play,
    Pause,
    Stop,
    SeekStart,
    Tempo(f64),
    Mode(Mode),
    Gain { track: usize, gain: f32 },
    Mute { track: usize, muted: bool },
    LaunchScene(usize),
}

/// Approximate independent telemetry values; never used for engine decisions.
pub struct Telemetry {
    pub playing: AtomicBool,
    pub beat: AtomicU64,
    pub scene: AtomicU32,
    pub peaks: [AtomicU32; TRACKS],
    pub callback_max_ns: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub device_fault: AtomicBool,
}
impl Default for Telemetry {
    fn default() -> Self {
        Self {
            playing: AtomicBool::new(false),
            beat: AtomicU64::new(0),
            scene: AtomicU32::new(0),
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            callback_max_ns: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            device_fault: AtomicBool::new(false),
        }
    }
}
impl Telemetry {
    pub fn beat(&self) -> f64 {
        f64::from_bits(self.beat.load(Relaxed))
    }
    pub fn peak(&self, track: usize) -> f32 {
        f32::from_bits(self.peaks[track].load(Relaxed))
    }
}

pub struct LiveEngine {
    rate: u32,
    media: [[Vec<Frame>; 2]; TRACKS],
    gains: [f32; TRACKS],
    muted: [bool; TRACKS],
    playing: bool,
    bpm: f64,
    beat: f64,
    mode: Mode,
    scene: usize,
    pending_scene: Option<(usize, f64)>,
    peaks: [f32; TRACKS],
}
impl LiveEngine {
    /// Allocate and synthesize all media before moving this engine into a callback.
    pub fn demo(rate: u32) -> Result<Self, crate::Invalid> {
        if !(8_000..=192_000).contains(&rate) {
            return Err(crate::Invalid("unsupported demo sample rate"));
        }
        let length = rate as usize * 2; // Four quarter-note beats at source tempo 120 BPM.
        let media = std::array::from_fn(|track| {
            std::array::from_fn(|scene| {
                (0..length)
                    .map(|i| {
                        let t = i as f64 / f64::from(rate);
                        let beat = t * 2.0;
                        let phase = beat.fract();
                        let tau = std::f64::consts::TAU;
                        let value = match track {
                            0 => {
                                let x = phase * 0.5;
                                (tau * (48.0 * x + 5.0 * (1.0 - (-x * 35.0).exp()))).sin()
                                    * (-x * 18.0).exp()
                                    * 0.35
                            }
                            1 => {
                                let x = (beat * 2.0).fract();
                                (tau * (scene as f64 * 110.0 + 880.0) * t).sin()
                                    * (1.0 - x).powi(8)
                                    * 0.09
                            }
                            2 => {
                                let freq = if scene == 0 { 55.0 } else { 73.416 };
                                (tau * freq * t).sin() * (1.0 - phase).powi(2) * 0.22
                            }
                            _ => (tau * 220.0 * t).sin() * (tau * 0.5 * t).sin().powi(2) * 0.05,
                        };
                        [value as f32; 2]
                    })
                    .collect()
            })
        });
        Ok(Self {
            rate,
            media,
            gains: [1.0; TRACKS],
            muted: [false; TRACKS],
            playing: false,
            bpm: 120.0,
            beat: 0.0,
            mode: Mode::Arrangement,
            scene: 0,
            pending_scene: None,
            peaks: [0.0; TRACKS],
        })
    }
    pub fn apply(&mut self, command: Command) {
        match command {
            Command::Play => self.playing = true,
            Command::Pause => self.playing = false,
            Command::Stop => {
                self.playing = false;
                self.beat = 0.0;
                self.pending_scene = None;
            }
            Command::SeekStart => {
                self.beat = 0.0;
                self.pending_scene = None;
            }
            Command::Tempo(bpm) if bpm.is_finite() && (20.0..=300.0).contains(&bpm) => {
                self.bpm = bpm
            }
            Command::Mode(mode) => {
                self.mode = mode;
                self.pending_scene = None;
            }
            Command::Gain { track, gain }
                if track < TRACKS && gain.is_finite() && (0.0..=2.0).contains(&gain) =>
            {
                self.gains[track] = gain
            }
            Command::Mute { track, muted } if track < TRACKS => self.muted[track] = muted,
            Command::LaunchScene(scene) if scene < 2 => {
                self.mode = Mode::Session;
                if !self.playing {
                    self.scene = scene;
                    self.pending_scene = None;
                } else {
                    self.pending_scene = Some((scene, (self.beat / 4.0).ceil() * 4.0));
                }
            }
            _ => {}
        }
    }
    pub fn begin_block(&mut self) {
        self.peaks.fill(0.0);
    }
    pub fn next_frame(&mut self) -> Frame {
        if !self.playing {
            return [0.0; 2];
        }
        if let Some((scene, boundary)) = self.pending_scene
            && self.beat >= boundary
        {
            self.scene = scene;
            self.pending_scene = None;
        }
        let scene = if self.mode == Mode::Arrangement {
            (self.beat / 4.0).floor() as usize % 2
        } else {
            self.scene
        };
        let position = self.beat.rem_euclid(4.0) * f64::from(self.rate) * 0.5;
        let index = position.floor() as usize;
        let fraction = position.fract() as f32;
        let mut result = [0.0; 2];
        for track in 0..TRACKS {
            if self.muted[track] {
                continue;
            }
            let source = &self.media[track][scene];
            let a = source[index % source.len()];
            let b = source[(index + 1) % source.len()];
            for channel in 0..2 {
                let sample =
                    (a[channel] + (b[channel] - a[channel]) * fraction) * self.gains[track];
                result[channel] += sample;
                self.peaks[track] = self.peaks[track].max(sample.abs());
            }
        }
        self.beat += self.bpm / (60.0 * f64::from(self.rate));
        // Bounded phase avoids long-running f64 precision degradation. All demo loops span 8 beats.
        if self.beat >= 8.0 {
            self.beat -= 8.0;
            if let Some((_, boundary)) = &mut self.pending_scene {
                *boundary -= 8.0;
            }
        }
        result.map(|sample| sample.clamp(-1.0, 1.0))
    }
    pub fn publish(&self, telemetry: &Telemetry) {
        telemetry.playing.store(self.playing, Relaxed);
        telemetry.beat.store(self.beat.to_bits(), Relaxed);
        telemetry.scene.store(
            if self.mode == Mode::Arrangement {
                (self.beat / 4.0).floor() as u32 % 2
            } else {
                self.scene as u32
            },
            Relaxed,
        );
        for (peak, value) in telemetry.peaks.iter().zip(self.peaks) {
            peak.store(value.to_bits(), Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_stop_and_seek() {
        let mut e = LiveEngine::demo(8_000).unwrap();
        assert_eq!(e.next_frame(), [0.0; 2]);
        e.apply(Command::Play);
        for _ in 0..100 {
            e.next_frame();
        }
        let beat = e.beat;
        e.apply(Command::Pause);
        assert_eq!(e.next_frame(), [0.0; 2]);
        assert_eq!(e.beat, beat);
        e.apply(Command::SeekStart);
        assert_eq!(e.beat, 0.0);
        e.apply(Command::Play);
        e.next_frame();
        e.apply(Command::Stop);
        assert!(!e.playing);
        assert_eq!(e.beat, 0.0);
    }
    #[test]
    fn launch_quantizes_and_latest_request_wins() {
        let mut e = LiveEngine::demo(8_000).unwrap();
        e.apply(Command::Mode(Mode::Session));
        e.apply(Command::Play);
        e.beat = 3.9999;
        e.apply(Command::LaunchScene(1));
        e.next_frame();
        assert_eq!(e.scene, 0);
        e.next_frame();
        assert_eq!(e.scene, 1);
        assert!(e.pending_scene.is_none());
        e.beat = 7.9;
        e.apply(Command::LaunchScene(0));
        e.apply(Command::LaunchScene(1));
        for _ in 0..500 {
            e.next_frame();
        }
        assert_eq!(e.scene, 1);
        assert!(e.pending_scene.is_none());
    }
    #[test]
    fn muted_tracks_silence_and_invalid_commands_are_ignored() {
        let mut e = LiveEngine::demo(8_000).unwrap();
        e.apply(Command::Tempo(f64::NAN));
        assert_eq!(e.bpm, 120.0);
        e.apply(Command::Gain {
            track: usize::MAX,
            gain: 1.0,
        });
        e.apply(Command::LaunchScene(9));
        for track in 0..TRACKS {
            e.apply(Command::Mute { track, muted: true });
        }
        e.apply(Command::Play);
        for _ in 0..100 {
            assert_eq!(e.next_frame(), [0.0; 2]);
        }
    }
    #[test]
    fn rendering_is_block_partition_independent() {
        let mut a = LiveEngine::demo(8_000).unwrap();
        let mut b = LiveEngine::demo(8_000).unwrap();
        a.apply(Command::Play);
        b.apply(Command::Play);
        let expected: Vec<_> = (0..2000).map(|_| a.next_frame()).collect();
        let mut actual = Vec::new();
        for length in [31, 997, 972] {
            b.begin_block();
            for _ in 0..length {
                actual.push(b.next_frame());
            }
        }
        assert_eq!(actual, expected);
    }
}
