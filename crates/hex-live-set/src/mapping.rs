mod guards;
use crate::{Diagnostic, Error, Imported, Options};
use guards::{guard_clip, guard_track};
use hex_project::*;
use roxmltree::{Document, Node};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Result<Node<'a, 'i>, Error> {
    let mut matches = n.children().filter(|c| c.has_tag_name(name));
    let found = matches
        .next()
        .ok_or(Error::Invalid("required field missing"))?;
    if matches.next().is_some() {
        return Err(Error::Invalid("duplicate field"));
    }
    Ok(found)
}
fn at<'a, 'i>(mut n: Node<'a, 'i>, path: &str) -> Result<Node<'a, 'i>, Error> {
    for name in path.split('/') {
        n = child(n, name)?;
    }
    Ok(n)
}
fn attr<'a>(n: Node<'a, '_>, name: &str) -> Result<&'a str, Error> {
    n.attribute(name)
        .ok_or(Error::Invalid("required attribute missing"))
}
fn value<'a>(n: Node<'a, '_>, path: &str) -> Result<&'a str, Error> {
    attr(at(n, path)?, "Value")
}
fn number(n: Node<'_, '_>, path: &str) -> Result<f64, Error> {
    numeric(value(n, path)?)
}
fn numeric(s: &str) -> Result<f64, Error> {
    let x: f64 = s.parse().map_err(|_| Error::Invalid("invalid number"))?;
    if !x.is_finite() {
        return Err(Error::Invalid("nonfinite number"));
    }
    Ok(x)
}
fn same(n: Node<'_, '_>, path: &str, expected: &str, reason: &'static str) -> Result<(), Error> {
    if value(n, path)? != expected {
        return Err(Error::Unsupported(reason));
    }
    Ok(())
}
fn empty(n: Node<'_, '_>, path: &str, reason: &'static str) -> Result<(), Error> {
    if at(n, path)?.children().any(|c| c.is_element()) {
        return Err(Error::Unsupported(reason));
    }
    Ok(())
}
fn beat(x: f64) -> Result<u64, Error> {
    let raw = x * 4294967296.0;
    if x < 0.0 || raw >= u64::MAX as f64 || !raw.is_finite() {
        return Err(Error::Invalid("beat out of range"));
    }
    // No invented fractional timing rounding: representable Q32.32 only.
    if raw.fract() != 0.0 {
        return Err(Error::Unsupported("beat precision"));
    }
    Ok(raw as u64)
}
fn seconds(x: f64, rate: u32) -> Result<u64, Error> {
    let frames = x * f64::from(rate);
    if x < 0.0 || !frames.is_finite() || frames >= u64::MAX as f64 {
        return Err(Error::Invalid("source time out of range"));
    }
    if frames.fract().abs() > 1e-6 {
        return Err(Error::Unsupported("fractional source frames"));
    }
    Ok(frames.round() as u64)
}
fn id(next: &mut u64) -> Result<u64, Error> {
    let result = *next;
    *next = next.checked_add(1).ok_or(Error::Limit("ids"))?;
    Ok(result)
}
fn local_path(base: &Path, relative: &str) -> Result<std::path::PathBuf, Error> {
    if relative.len() > 4096 || relative.contains(['\\', ':']) {
        return Err(Error::Asset("unsupported relative path"));
    }
    let path = Path::new(relative);
    if relative.is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::Asset("relative path must stay within set directory"));
    }
    let root = base
        .canonicalize()
        .map_err(|_| Error::Asset("set directory unavailable"))?;
    let mut current = root.clone();
    for part in path.components() {
        current.push(part.as_os_str());
        let m =
            std::fs::symlink_metadata(&current).map_err(|_| Error::Asset("local media missing"))?;
        if m.file_type().is_symlink() {
            return Err(Error::Asset("media symlink refused"));
        }
    }
    if !current.is_file() {
        return Err(Error::Asset("media must be a regular file"));
    }
    let canonical = current
        .canonicalize()
        .map_err(|_| Error::Asset("local media unavailable"))?;
    if !canonical.starts_with(root) {
        return Err(Error::Asset("media outside set directory"));
    }
    Ok(canonical)
}

pub fn map(
    doc: &Document<'_>,
    base: &Path,
    rate: u32,
    options: Options,
) -> Result<Imported, Error> {
    let root = doc.root_element();
    if !root.has_tag_name("Ableton")
        || attr(root, "MajorVersion")? != "5"
        || attr(root, "MinorVersion")? != "12.0_12402"
        || attr(root, "SchemaChangeCount")? != "5"
    {
        return Err(Error::Unsupported("live document version"));
    }
    if attr(root, "Creator")? != "Ableton Live 12.4.6"
        || attr(root, "Revision")? != "0de5c8fa9a692293676cb7700a15afb2046ee1f7"
    {
        return Err(Error::Unsupported("reference build"));
    }
    let set = child(root, "LiveSet")?;
    let main = child(set, "MainTrack")?;
    let mixer = at(main, "DeviceChain/Mixer")?;
    same(
        mixer,
        "TimeSignature/Manual",
        "201",
        "only evidenced 4/4 meter is supported",
    )?;
    same(mixer, "Pan/Manual", "0", "main pan")?;
    same(mixer, "CrossFade/Manual", "0", "crossfade")?;
    same(mixer, "GlobalGrooveAmount/Manual", "0", "groove")?;
    same(set, "Transport/LoopOn", "false", "transport loop")?;
    empty(set, "GroovePool/Grooves", "grooves")?;
    empty(set, "SignalModulations", "modulation")?;
    empty(set, "TuningSystems", "tuning system")?;
    same(mixer, "Speaker/Manual", "true", "main mute")?;
    let tempo_bpm = number(mixer, "Tempo/Manual")?;
    let micro = tempo_bpm * 1_000_000.0;
    if micro < 1.0 || micro > u32::MAX as f64 || micro.fract() != 0.0 {
        return Err(Error::Unsupported("tempo precision or range"));
    }
    let tempo = Tempo::new(micro as u32)?;
    let mut snapshot = Project::new(rate)?.snapshot();
    snapshot.tempo_points = vec![TempoPoint::new(BeatTime::ZERO, tempo)];
    snapshot.master_gain = number(mixer, "Volume/Manual")? as f32;
    let timing = TempoMap::constant(rate, tempo)?;
    guard_track(main, true, tempo_bpm)?;
    // Cue/prehear output is not imported; reject authored content/devices there.
    guard_track(child(set, "PreHearTrack")?, false, tempo_bpm)?;
    let mut next = 1u64;
    let mut media = BTreeMap::new();
    let mut diagnostics = vec![Diagnostic::OriginalDocumentRetained];
    let mut clips_seen = 0usize;
    let mut note_count = 0usize;
    let mut total_frames = 0usize;
    for track in child(set, "Tracks")?.children().filter(|n| n.is_element()) {
        if snapshot.tracks.len() >= 32 {
            return Err(Error::Limit("tracks"));
        }
        let audio = track.has_tag_name("AudioTrack");
        if !audio && !track.has_tag_name("MidiTrack") {
            return Err(Error::Unsupported("track kind"));
        }
        guard_track(track, false, tempo_bpm)?;
        same(
            track,
            "DeviceChain/MainSequencer/Recorder/IsArmed",
            "false",
            "armed track",
        )?;
        same(
            track,
            "DeviceChain/MainSequencer/MonitoringEnum",
            if audio { "2" } else { "1" },
            "monitoring mode",
        )?;
        same(
            track,
            "DeviceChain/AudioOutputRouting/Target",
            "AudioOut/Main",
            "audio routing",
        )?;
        same(
            track,
            "DeviceChain/MidiOutputRouting/Target",
            "MidiOut/None",
            "midi output routing",
        )?;
        same(
            track,
            "DeviceChain/MainSequencer/On/Manual",
            "true",
            "sequencer disabled",
        )?;
        let m = at(track, "DeviceChain/Mixer")?;
        let pan = number(m, "Pan/Manual")?;
        if !(-1.0..=1.0).contains(&pan) {
            return Err(Error::Invalid("pan range"));
        }
        if pan != 0.0 {
            if !options.accept_hex_balance {
                return Err(Error::Unsupported(
                    "nonneutral pan requires explicit hex balance consent",
                ));
            }
            if !diagnostics.contains(&Diagnostic::HexBalanceLaw) {
                diagnostics.push(Diagnostic::HexBalanceLaw);
            }
        }
        let muted = match value(m, "Speaker/Manual")? {
            "true" => false,
            "false" => true,
            _ => return Err(Error::Invalid("mute boolean")),
        };
        let track_id = id(&mut next)?;
        let mut t = TrackSnapshot {
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: value(track, "Name/EffectiveName")?.to_owned(),
            gain: number(m, "Volume/Manual")? as f32,
            balance: pan as f32,
            muted,
            soloed: false,
            devices: vec![],
            instrument: None,
            instrument_position: None,
            sends: vec![],
            content: TrackContentSnapshot::Hybrid {
                audio_clips: vec![],
                midi_clips: vec![],
            },
        };
        let container = if audio {
            "DeviceChain/MainSequencer/Sample/ArrangerAutomation/Events"
        } else {
            "DeviceChain/MainSequencer/ClipTimeable/ArrangerAutomation/Events"
        };
        for c in at(track, container)?.children().filter(|n| n.is_element()) {
            clips_seen += 1;
            if clips_seen > 64 {
                return Err(Error::Limit("arrangement clips"));
            }
            if !c.has_tag_name(if audio { "AudioClip" } else { "MidiClip" }) {
                return Err(Error::Unsupported("arrangement event"));
            }
            guard_clip(c, audio)?;
            let start = beat(number(c, "CurrentStart")?)?;
            let end = beat(number(c, "CurrentEnd")?)?;
            let length = end
                .checked_sub(start)
                .filter(|n| *n > 0)
                .ok_or(Error::Invalid("clip interval"))?;
            if beat(numeric(attr(c, "Time")?)?)? != start {
                return Err(Error::Unsupported("clip placement disagreement"));
            }
            let source_start = number(c, "Loop/LoopStart")?;
            let source_end = number(c, "Loop/LoopEnd")?;
            let clip_id = id(&mut next)?;
            let TrackContentSnapshot::Hybrid {
                audio_clips,
                midi_clips,
            } = &mut t.content
            else {
                return Err(Error::Invalid("track content"));
            };
            if audio {
                same(c, "IsWarped", "false", "warped audio")?;
                same(c, "Fade", "false", "audio fades")?;
                same(c, "Fades/FadeInLength", "0", "audio fades")?;
                same(c, "Fades/FadeOutLength", "0", "audio fades")?;
                same(c, "PitchCoarse", "0", "audio transpose")?;
                same(c, "PitchFine", "0", "audio detune")?;
                same(c, "SampleVolume", "1", "clip gain")?;
                same(c, "IsSongTempoLeader", "false", "tempo leader")?;
                let file = at(c, "SampleRef/FileRef")?;
                same(file, "RelativePathType", "3", "file reference kind")?;
                same(file, "Type", "2", "file reference type")?;
                same(file, "LivePackName", "", "live pack")?;
                same(file, "LivePackId", "", "live pack")?;
                let path = local_path(base, value(file, "RelativePath")?)?;
                // Never fall back to saved absolute Path; no remote retrieval.
                let decoded = hex_media::decode_wav_file(
                    &path,
                    hex_media::Limits::new(64 * 1024 * 1024, 8_000_000),
                )
                .map_err(|_| Error::Asset("local wav decode failed"))?;
                if decoded.sample_rate() != rate {
                    return Err(Error::Unsupported(
                        "initial subset requires project-rate wav",
                    ));
                }
                if number(c, "SampleRef/DefaultSampleRate")? != f64::from(rate)
                    || number(c, "SampleRef/DefaultDuration")? != decoded.frames() as f64
                {
                    return Err(Error::Asset("saved source metadata mismatch"));
                }
                total_frames = total_frames
                    .checked_add(decoded.frames())
                    .ok_or(Error::Limit("media frames"))?;
                if total_frames > 8_000_000 {
                    return Err(Error::Limit("total resident media"));
                }
                let offset = seconds(source_start, rate)?;
                let source_length = seconds(source_end, rate)?
                    .checked_sub(offset)
                    .filter(|n| *n > 0)
                    .ok_or(Error::Invalid("audio source interval"))?;
                let start_frame = timing.beat_to_frame(BeatTime::from_raw(start))?;
                let duration = timing
                    .beat_to_frame(BeatTime::from_raw(end))?
                    .checked_sub(start_frame)
                    .filter(|n| *n > 0)
                    .ok_or(Error::Invalid("audio duration"))?;
                if duration != source_length {
                    return Err(Error::Unsupported(
                        "one-shot duration must match source markers",
                    ));
                }
                let asset = id(&mut next)?;
                snapshot.assets.push(AssetSnapshot {
                    id: asset,
                    path,
                    source_sample_rate: rate,
                    source_frames: decoded.frames() as u64,
                    playback_frames: decoded.frames() as u64,
                    conversion_version: IDENTITY_CONVERSION,
                });
                // AssetId is opaque; materialize bank keys after canonical validation.
                media.insert(asset, decoded.into_audio());
                audio_clips.push(ClipSnapshot {
                    id: clip_id,
                    asset,
                    start: start_frame,
                    duration,
                    source_offset: offset,
                    source_length,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(duration)?,
                });
            } else {
                if !diagnostics.contains(&Diagnostic::DeviceFreeMidiIsSilent) {
                    diagnostics.push(Diagnostic::DeviceFreeMidiIsSilent);
                }
                let a = beat(source_start)?;
                let b = beat(source_end)?;
                if a != 0 || b != length {
                    return Err(Error::Unsupported(
                        "midi source markers must span finite region",
                    ));
                }
                let mut notes = vec![];
                for key in at(c, "Notes/KeyTracks")?
                    .children()
                    .filter(|n| n.is_element())
                {
                    if !key.has_tag_name("KeyTrack") {
                        return Err(Error::Unsupported("note container"));
                    }
                    let pitch = number(key, "MidiKey")?;
                    if !(0.0..=127.0).contains(&pitch) || pitch.fract() != 0.0 {
                        return Err(Error::Invalid("note pitch"));
                    }
                    for note in child(key, "Notes")?.children().filter(|n| n.is_element()) {
                        if !note.has_tag_name("MidiNoteEvent") {
                            return Err(Error::Unsupported("note event"));
                        }
                        if note.children().any(|n| n.is_element()) {
                            return Err(Error::Unsupported("nested note behavior"));
                        }
                        if note.attributes().any(|a| {
                            !["Time", "Duration", "Velocity", "OffVelocity", "NoteId"]
                                .contains(&a.name())
                        }) {
                            return Err(Error::Unsupported("note expression or probability"));
                        }
                        if attr(note, "OffVelocity")? != "0" {
                            return Err(Error::Unsupported("note release velocity"));
                        }
                        let velocity = numeric(attr(note, "Velocity")?)?;
                        if !(1.0..=127.0).contains(&velocity) || velocity.fract() != 0.0 {
                            return Err(Error::Unsupported("note velocity"));
                        }
                        note_count += 1;
                        if note_count > 65_536 {
                            return Err(Error::Limit("notes"));
                        }
                        notes.push(MidiNoteSnapshot {
                            id: id(&mut next)?,
                            start_raw: beat(numeric(attr(note, "Time")?)?)?,
                            duration_raw: beat(numeric(attr(note, "Duration")?)?)?,
                            pitch: pitch as u8,
                            channel: 0,
                            velocity: velocity as u8,
                        });
                    }
                }
                midi_clips.push(MidiClipSnapshot {
                    id: clip_id,
                    start_raw: start,
                    length_raw: length,
                    source_length_raw: b,
                    source_start_raw: 0,
                    loop_start_raw: 0,
                    loop_end_raw: b,
                    looping: false,
                    notes,
                });
            }
        }
        snapshot.tracks.push(t);
    }
    // Reject Session, freeze or other unhandled clip locations instead of losing music.
    if doc
        .descendants()
        .filter(|n| n.has_tag_name("AudioClip") || n.has_tag_name("MidiClip"))
        .count()
        != clips_seen
    {
        return Err(Error::Unsupported("session or alternate clip location"));
    }
    snapshot.next_id = next;
    let project = Project::from_snapshot(snapshot)?;
    let bank = project
        .assets()
        .map(|asset| {
            let audio = media
                .remove(&asset.id().value())
                .ok_or(Error::Asset("asset bank mismatch"))?;
            Ok((asset.id(), audio))
        })
        .collect::<Result<_, Error>>()?;
    Ok(Imported {
        project,
        media: bank,
        diagnostics,
    })
}
