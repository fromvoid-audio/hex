//! Reproducible effectful Return stem listening fixture (Pulse #7220 / #2388).
//!
//! Creates a deterministic 48 kHz project with:
//! - Track 1 (`lead-selected`, gain 0.85, pan -0.35): held MIDI chord (`C4`+`E4`, 0.0..1.5 s) -> Return A send 0.65
//! - Track 2 (`pad-unselected`, gain 0.70, pan +0.40): held MIDI note (`G3`, 0.5..2.0 s) -> Return A send 0.50
//! - Return A (`reverb-return`, gain 0.90): native `hex.reverb` (100% wet)
//!
//! Saves and reopens `return-listening.hex`, then exports Main (`main.wav`), the selected
//! Track 1 stem, and the separate Return A stem over 4.0 seconds (including 2.0 seconds
//! of reverb release tail). Verifies non-zero tail
//! energy and distinct Main vs Return output, and prints a concise user listening checklist.
//!
//! Usage:
//! `cargo run --release -p hex-export --example effectful_return_stem_fixture -- [output_dir]`
use hex_engine::{OutputCaptureId, PreparedRevision, SineTable, default_native_device_state};
use hex_export::{ExportControl, Stem, render_prepared_new, render_stems_new};
use hex_lattice_contract::{DeviceTypeId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project, TrackDeviceSlot};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const RATE: u32 = 48_000;
const SECONDS: u64 = 4;
const FRAMES: u64 = RATE as u64 * SECONDS;

fn wav_metrics(path: &Path) -> Result<(f32, f32, u64), Box<dyn Error>> {
    let decoded = hex_media::decode_wav_file(
        path,
        hex_media::Limits::new(16 * 1024 * 1024, FRAMES as usize),
    )?;
    let frames = decoded.audio().frames();
    let mut peak = 0.0_f32;
    let mut tail_peak = 0.0_f32;
    let tail_start = (RATE as usize) * 2 + (RATE as usize) / 2; // 2.5 s (well after all notes off)
    let mut digest = 0xcbf29ce484222325_u64;
    for (idx, frame) in frames.iter().enumerate() {
        let mag = frame[0].abs().max(frame[1].abs());
        peak = peak.max(mag);
        if idx >= tail_start {
            tail_peak = tail_peak.max(mag);
        }
        for sample in frame {
            for byte in sample.to_le_bytes() {
                digest = (digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
    }
    Ok((peak, tail_peak, digest))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let out_dir = match args.next() {
        Some(dir) => PathBuf::from(dir),
        None => {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            std::env::temp_dir().join(format!(
                "hex-return-stem-fixture-{}-{nonce}",
                std::process::id()
            ))
        }
    };
    if args.next().is_some() {
        return Err("usage: effectful_return_stem_fixture [output_dir]".into());
    }
    fs::create_dir_all(&out_dir)?;

    let mut editor = Editor::new(Project::new(RATE)?, 32)?;
    let Created::Track(track_lead) = editor.execute(Command::InsertTrackWithDevice {
        name: "lead-selected".into(),
        before_track: None,
        slot: TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("lead track creation failed".into());
    };
    let Created::Track(track_pad) = editor.execute(Command::InsertTrackWithDevice {
        name: "pad-unselected".into(),
        before_track: None,
        slot: TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state()?,
    })?
    else {
        return Err("pad track creation failed".into());
    };
    editor.execute(Command::Gain {
        track: track_lead,
        gain: 0.85,
    })?;
    editor.execute(Command::Balance {
        track: track_lead,
        balance: -0.35,
    })?;
    editor.execute(Command::Gain {
        track: track_pad,
        gain: 0.70,
    })?;
    editor.execute(Command::Balance {
        track: track_pad,
        balance: 0.40,
    })?;

    // Held notes on Track 1 (C4 + E4, 0.0..1.5 s @ 120 BPM = 0..3 beats) and Track 2 (G3, 0.5..2.0 s = 1..4 beats).
    let Created::Clip(clip_lead) = editor.execute(Command::PlaceMidiClip {
        track: track_lead,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })?
    else {
        return Err("lead midi clip failed".into());
    };
    for (pitch, velocity) in [(60, 108), (64, 96)] {
        editor.execute(Command::AddMidiNote {
            clip: clip_lead,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(3)?,
            pitch,
            channel: 0,
            velocity,
        })?;
    }
    let Created::Clip(clip_pad) = editor.execute(Command::PlaceMidiClip {
        track: track_pad,
        start: BeatTime::from_quarters(1)?,
        length: BeatTime::from_quarters(4)?,
    })?
    else {
        return Err("pad midi clip failed".into());
    };
    editor.execute(Command::AddMidiNote {
        clip: clip_pad,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(3)?,
        pitch: 55,
        channel: 0,
        velocity: 100,
    })?;

    // Shared Return A with native hex.reverb.
    let Created::ReturnTrack(return_bus) = editor.execute(Command::AddReturnTrack {
        name: "reverb-return".into(),
    })?
    else {
        return Err("return track failed".into());
    };
    editor.execute(Command::ReturnGain {
        return_track: return_bus,
        gain: 0.90,
    })?;
    let reverb_state = default_native_device_state(DeviceTypeId::from_static("hex.reverb"))?;
    let reverb_state = hex_engine::patch_native_device_parameter(
        &reverb_state,
        hex_devices::reverb::parameter::DRY_WET,
        ParameterValue::Float(100.0),
    )?;
    editor.execute(Command::InsertReturnDevice {
        return_track: return_bus,
        before: None,
        state: reverb_state,
    })?;
    editor.execute(Command::SetReturnSend {
        track: track_lead,
        return_track: return_bus,
        gain: 0.65,
    })?;
    editor.execute(Command::SetReturnSend {
        track: track_pad,
        return_track: return_bus,
        gain: 0.50,
    })?;

    let project_path = out_dir.join("return-listening.hex");
    hex_storage::save_new(&project_path, editor.project())?;
    let reopened = hex_storage::load(&project_path)?;
    if &reopened != editor.project() {
        return Err("reopened listening project differs from the authored project".into());
    }
    let revision = Arc::new(PreparedRevision::prepare(
        &reopened,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);

    let main_path = out_dir.join("main.wav");
    let stems_dir = out_dir.join("stems");
    let main_report = render_prepared_new(&main_path, Arc::clone(&revision), 0, FRAMES)?;
    let stems = [
        Stem {
            id: OutputCaptureId::Track(track_lead),
            name: "lead-selected".into(),
        },
        Stem {
            id: OutputCaptureId::Return(return_bus),
            name: "reverb-return".into(),
        },
    ];
    let control = ExportControl::default();
    let stem_report = render_stems_new(&stems_dir, revision, 0, FRAMES, &stems, &control)?;

    let (main_peak, main_tail, main_digest) = wav_metrics(&main_path)?;
    let lead_path = stems_dir.join(format!("track-{}-lead-selected.wav", track_lead.value()));
    let ret_path = stems_dir.join(format!("return-{}-reverb-return.wav", return_bus.value()));
    let (lead_peak, lead_tail, lead_digest) = wav_metrics(&lead_path)?;
    let (ret_peak, ret_tail, ret_digest) = wav_metrics(&ret_path)?;

    if main_peak <= 0.01 || lead_peak <= 0.01 || ret_peak <= 0.005 || ret_tail <= 1e-4 {
        return Err(format!(
            "unexpected silent fixture: main={main_peak} lead={lead_peak} ret={ret_peak} ret_tail={ret_tail}"
        )
        .into());
    }
    if main_digest == ret_digest || main_digest == lead_digest || lead_digest == ret_digest {
        return Err("main, lead stem, and return stem must be distinct".into());
    }

    println!("fixture_dir={}", out_dir.display());
    println!("project={}", project_path.display());
    println!("effect=hex.reverb rate={RATE} duration_s={SECONDS} frames={FRAMES}");
    println!(
        "main.wav frames={} peak={main_peak:.5} tail_peak_after_2.5s={main_tail:.5} digest={main_digest:016x}",
        main_report.frames
    );
    println!(
        "{} peak={lead_peak:.5} tail_peak_after_2.5s={lead_tail:.5} digest={lead_digest:016x}",
        lead_path.display()
    );
    println!(
        "{} peak={ret_peak:.5} tail_peak_after_2.5s={ret_tail:.5} digest={ret_digest:016x} batch_frames={}",
        ret_path.display(),
        stem_report.frames
    );
    println!("--- user listening checklist (#2388) ---");
    println!(
        "1. Listen to main.wav: confirm left-panned C4+E4 lead (0..1.5s), right-panned G3 pad (0.5..2.0s), and shared reverb tail decaying smoothly past 2.5s without clicks."
    );
    println!(
        "2. Listen to the lead stem: confirm dry left-panned lead only (no pad, no reverb tail)."
    );
    println!(
        "3. Listen to the return stem: confirm wet reverb fed by BOTH selected lead AND unselected pad, with smooth release tail past 2.5s and clean source labeling."
    );
    Ok(())
}
