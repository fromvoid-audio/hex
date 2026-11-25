//! Integration tests for FLAC export, target sample-rate conversion, opt-in
//! peak/loudness normalization, PDC preroll, TPDF silence preservation, and
//! atomic overwrite safety.

use hex_engine::{Audio, PreparedRevision};
use hex_export::{
    ExportControl, ExportFormat, ExportJobSpec, ExportNormalization, ExportOptions, ExportSignal,
    Publication, Stem, measure_integrated_lufs, render_prepared_with_options,
};
use hex_media::{Limits, MediaFormat, decode_audio_file, detect_format};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    f64::consts::PI,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-export-flac-src-norm-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temp test dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sine_revision(
    sample_rate: u32,
    frames: usize,
    hz: f64,
    amplitude: f32,
) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(sample_rate).unwrap(), 8).unwrap();
    let Created::Imported { asset, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "tone".into(),
            path: PathBuf::from("tone.wav"),
            sample_rate,
            frames: frames as u64,
        })
        .unwrap()
    else {
        panic!("expected imported audio track");
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let pcm: Vec<[f32; 2]> = (0..frames)
        .map(|i| {
            let s = ((2.0 * PI * hz * i as f64) / f64::from(sample_rate)).sin() as f32 * amplitude;
            [s, -s * 0.5]
        })
        .collect();
    let mut media = BTreeMap::new();
    media.insert(asset, Audio::new(sample_rate, pcm).unwrap());
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

fn silent_revision(sample_rate: u32, frames: usize) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(sample_rate).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "silence".into(),
            path: PathBuf::from("silence.wav"),
            sample_rate,
            frames: frames as u64,
        })
        .unwrap()
    else {
        panic!("expected imported audio track");
    };
    let mut media = BTreeMap::new();
    media.insert(
        asset,
        Audio::new(sample_rate, vec![[0.0, 0.0]; frames]).unwrap(),
    );
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

#[test]
fn flac16_and_flac24_export_round_trip_and_preserve_digital_silence() {
    let dir = TempDir::new();
    // > 4096 frames to span multiple FLAC blocks (4096 + tail block).
    let frames = 5_000_usize;
    let revision = sine_revision(48_000, frames, 440.0, 0.5);

    for (name, format, tol) in [
        ("out16.flac", ExportFormat::Flac16, 1.0e-3_f32),
        ("out24.flac", ExportFormat::Flac24, 1.0e-5_f32),
    ] {
        let dest = dir.path().join(name);
        let report = render_prepared_with_options(
            ExportJobSpec {
                path: &dest,
                revision: Arc::clone(&revision),
                resources: None,
                start: 0,
                frames: frames as u64,
                signal: ExportSignal::Main,
                format,
                control: &ExportControl::default(),
                publication: &Publication::CreateNew,
            },
            ExportOptions::default(),
            None,
        )
        .unwrap();
        assert_eq!(report.sample_rate, 48_000);
        assert_eq!(report.frames, frames as u64);
        assert_eq!(report.clipped_samples, 0);
        assert_eq!(report.applied_gain_linear, 1.0);
        assert_eq!(report.applied_gain_db, 0.0);

        let raw = fs::read(&dest).unwrap();
        assert_eq!(detect_format(&raw), Some(MediaFormat::Flac));
        let decoded = decode_audio_file(&dest, Limits::new(4 * 1024 * 1024, 16_384)).unwrap();
        assert_eq!(decoded.sample_rate(), 48_000);
        assert_eq!(decoded.frames(), frames);
        for (i, &[l, r]) in decoded.audio().frames().iter().enumerate() {
            let expected = ((2.0 * PI * 440.0 * i as f64) / 48_000.0).sin() as f32 * 0.5;
            assert!(
                (l - expected).abs() <= tol,
                "{format:?} L@{i}: {l} vs {expected}"
            );
            assert!(
                (r - (-expected * 0.5)).abs() <= tol,
                "{format:?} R@{i}: {r} vs {}",
                -expected * 0.5
            );
        }
    }

    // TPDF dither in Flac16 and rounding in Flac24 must preserve exact digital silence (0.0 -> 0).
    let silent = silent_revision(48_000, 512);
    for (name, format) in [
        ("silent16.flac", ExportFormat::Flac16),
        ("silent24.flac", ExportFormat::Flac24),
    ] {
        let dest = dir.path().join(name);
        render_prepared_with_options(
            ExportJobSpec {
                path: &dest,
                revision: Arc::clone(&silent),
                resources: None,
                start: 0,
                frames: 512,
                signal: ExportSignal::Main,
                format,
                control: &ExportControl::default(),
                publication: &Publication::CreateNew,
            },
            ExportOptions::default(),
            None,
        )
        .unwrap();
        let decoded = decode_audio_file(&dest, Limits::new(1024 * 1024, 2048)).unwrap();
        assert!(
            decoded.audio().frames().iter().all(|&f| f == [0.0, 0.0]),
            "{format:?} did not preserve exact digital silence"
        );
    }
}

#[test]
fn target_src_and_peak_loudness_normalization_report_exact_applied_gain() {
    let dir = TempDir::new();
    // 1 second of 1 kHz stereo sine at -12 dBFS (~0.2512).
    let source_rate = 48_000_u32;
    let source_frames = 48_000_usize;
    let revision = sine_revision(source_rate, source_frames, 1000.0, 0.25);

    // 1. Target SRC 48 kHz -> 44.1 kHz + peak normalization to -1.0 dBFS -> FLAC24.
    let peak_path = dir.path().join("resampled-peak.flac");
    let peak_report = render_prepared_with_options(
        ExportJobSpec {
            path: &peak_path,
            revision: Arc::clone(&revision),
            resources: None,
            start: 0,
            frames: source_frames as u64,
            signal: ExportSignal::Main,
            format: ExportFormat::Flac24,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        ExportOptions {
            target_sample_rate: Some(44_100),
            normalization: ExportNormalization::Peak { target_dbfs: -1.0 },
        },
        None,
    )
    .unwrap();
    assert_eq!(peak_report.sample_rate, 44_100);
    assert_eq!(peak_report.frames, 44_100);
    assert!(peak_report.applied_gain_db > 10.0 && peak_report.applied_gain_db < 12.0);
    assert!(
        (peak_report.applied_gain_db - 20.0 * peak_report.applied_gain_linear.log10()).abs()
            < 1e-12
    );
    let decoded_peak = decode_audio_file(&peak_path, Limits::new(8 * 1024 * 1024, 65_536)).unwrap();
    assert_eq!(decoded_peak.sample_rate(), 44_100);
    assert_eq!(decoded_peak.frames(), 44_100);
    let measured_peak = decoded_peak
        .audio()
        .frames()
        .iter()
        .flat_map(|f| [f[0].abs(), f[1].abs()])
        .fold(0.0_f32, f32::max);
    let target_linear = 10.0_f64.powf(-1.0 / 20.0);
    assert!(
        (f64::from(measured_peak) - target_linear).abs() < 1e-3,
        "measured peak {measured_peak} vs {target_linear}"
    );

    // 2. Loudness normalization to -14 LUFS with -1.0 dBFS ceiling -> Float32 WAV.
    let lufs_path = dir.path().join("normalized-lufs.wav");
    let lufs_report = render_prepared_with_options(
        ExportJobSpec {
            path: &lufs_path,
            revision: Arc::clone(&revision),
            resources: None,
            start: 0,
            frames: source_frames as u64,
            signal: ExportSignal::Main,
            format: ExportFormat::Float32,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        ExportOptions {
            target_sample_rate: None,
            normalization: ExportNormalization::Loudness {
                target_lufs: -14.0,
                true_peak_ceiling_dbfs: Some(-1.0),
            },
        },
        None,
    )
    .unwrap();
    assert_eq!(lufs_report.sample_rate, 48_000);
    assert!(lufs_report.applied_gain_linear > 1.0);
    let decoded_lufs = decode_audio_file(&lufs_path, Limits::new(8 * 1024 * 1024, 65_536)).unwrap();
    let measured_lufs = measure_integrated_lufs(decoded_lufs.audio().frames(), 48_000).unwrap();
    assert!(
        (measured_lufs - (-14.0)).abs() < 0.1,
        "measured LUFS {measured_lufs}"
    );

    // 3. Atomic overwrite safety: CreateNew refuses existing destination without touching it.
    let before = fs::read(&peak_path).unwrap();
    let collision = render_prepared_with_options(
        ExportJobSpec {
            path: &peak_path,
            revision,
            resources: None,
            start: 0,
            frames: 512,
            signal: ExportSignal::Main,
            format: ExportFormat::Flac16,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        ExportOptions::default(),
        None,
    );
    assert!(collision.is_err());
    assert_eq!(fs::read(&peak_path).unwrap(), before);
}

#[test]
fn flac_stem_batch_writes_dot_flac_files() {
    let dir = TempDir::new();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported {
        track, asset, clip, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "lead".into(),
            path: PathBuf::from("lead.wav"),
            sample_rate: 48_000,
            frames: 256,
        })
        .unwrap()
    else {
        panic!("expected track");
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let mut media = BTreeMap::new();
    media.insert(asset, Audio::new(48_000, vec![[0.25, -0.25]; 256]).unwrap());
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let batch = dir.path().join("flac-stems");
    let report = render_prepared_with_options(
        ExportJobSpec {
            path: &batch,
            revision,
            resources: None,
            start: 0,
            frames: 256,
            signal: ExportSignal::Stems(vec![Stem {
                id: hex_engine::OutputCaptureId::Track(track),
                name: "lead".into(),
            }]),
            format: ExportFormat::Flac24,
            control: &ExportControl::default(),
            publication: &Publication::CreateNew,
        },
        ExportOptions::default(),
        None,
    )
    .unwrap();
    assert_eq!(report.frames, 256);
    let entries: Vec<_> = fs::read_dir(&batch)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].extension().and_then(|e| e.to_str()),
        Some("flac")
    );
    let decoded = decode_audio_file(&entries[0], Limits::new(1024 * 1024, 1024)).unwrap();
    assert_eq!(decoded.frames(), 256);
    assert!((decoded.audio().frames()[0][0] - 0.25).abs() < 1e-5);
}
