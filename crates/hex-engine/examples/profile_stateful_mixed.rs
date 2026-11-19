//! Sustained mixed Arrangement playback with stateful effects and identical-revision
//! handoffs. The mixed-tail modes also exercise bypass and detached effects.
//! This is a headless timing fixture, not a physical device deadline.
//! `--companion` gives each Operator a looping, shaped envelope.
//! `--asymmetric` also gives the fourth operator a different sustain level.
//! `--distinct` gives all four operators different sustain levels.
#[allow(dead_code)]
#[path = "support/operator_feedback_fixture.rs"]
mod operator_fixture;

use hex_devices::operator_state::OPERATOR_METADATA;
use hex_devices::{compressor, gate, saturator, utility};
use hex_engine::{Audio, PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{
    AssetSnapshot, BeatTime, ClipFade, ClipGain, ClipSnapshot, DeviceSnapshot, IDENTITY_CONVERSION,
    Project, TrackContentSnapshot,
};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const RATE: u32 = 48_000;
const SOURCE: usize = 512;
const TRACKS: usize = 100;
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompanionProfile {
    Default,
    Uniform,
    Asymmetric,
    Distinct,
}
impl CompanionProfile {
    fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "--companion" => Some(Self::Uniform),
            "--asymmetric" => Some(Self::Asymmetric),
            "--distinct" => Some(Self::Distinct),
            _ => None,
        }
    }
}

pub(crate) fn fixture(
    tracks: usize,
    devices: usize,
) -> Result<(Project, BTreeMap<hex_project::AssetId, Audio>)> {
    fixture_with_companion(tracks, devices, CompanionProfile::Default)
}

fn fixture_with_companion(
    tracks: usize,
    devices: usize,
    profile: CompanionProfile,
) -> Result<(Project, BTreeMap<hex_project::AssetId, Audio>)> {
    let state = operator_fixture::state_with_operators(
        OPERATOR_METADATA.default_state()?,
        0.24,
        0.82,
        true,
    )?;
    let source = operator_fixture::project(tracks, 4, &state)?;
    let mut snapshot = source.project.snapshot();
    let asset = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.assets.push(AssetSnapshot {
        original_source: None,
        id: asset,
        path: "memory/stateful-mixed.wav".into(),
        source_sample_rate: RATE,
        source_frames: SOURCE as u64,
        playback_frames: SOURCE as u64,
        conversion_version: IDENTITY_CONVERSION,
    });
    let states = [
        gate::GATE_DEFINITION.default_state()?,
        compressor::COMPRESSOR_DEFINITION.default_state()?,
        saturator::SATURATOR_DEFINITION.default_state()?,
    ];
    for (index, track) in snapshot.tracks.iter_mut().enumerate() {
        track.gain = 0.01;
        if index % 2 == 0 {
            let clip = snapshot.next_id;
            snapshot.next_id += 1;
            track.instrument = None;
            track.instrument_position = None;
            track.content = TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "mixed audio".into(),
                flavor: Default::default(),
                edit: None,
                gain: ClipGain::UNITY,
                id: clip,
                asset,
                start: 0,
                duration: u64::from(RATE) * 140,
                source_offset: (index % 16) as u64,
                source_length: SOURCE as u64 - (index % 16) as u64,
                source_phase: 0,
                looping: true,
                fade: ClipFade::neutral(u64::from(RATE) * 140)?,
            }]);
        } else {
            let clips = match &mut track.content {
                TrackContentSnapshot::Instrument(clips)
                | TrackContentSnapshot::Hybrid {
                    midi_clips: clips, ..
                } => clips,
                TrackContentSnapshot::Audio(_) => return Err("missing fixture midi clips".into()),
            };
            let clip = clips.first_mut().ok_or("missing fixture midi clip")?;
            let length = BeatTime::from_quarters(256)?.raw();
            clip.length_raw = length;
            clip.source_length_raw = length;
            clip.loop_end_raw = length;
            for note in &mut clip.notes {
                note.duration_raw = BeatTime::from_quarters(240)?.raw();
            }
        }
        for device in 0..devices {
            track.devices.push(DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state: states[device % states.len()].clone(),
            });
            snapshot.next_id += 1;
        }
    }
    let audio_tracks: Vec<_> = snapshot
        .tracks
        .iter()
        .step_by(2)
        .map(|track| track.id)
        .collect();
    snapshot
        .native_sources
        .retain(|source| !source.receiver.is_some_and(|id| audio_tracks.contains(&id)));
    if profile != CompanionProfile::Default {
        for source in &mut snapshot.native_sources {
            let instrument = source
                .devices
                .get_mut(source.instrument_index)
                .ok_or("missing fixture source instrument")?;
            for operator in 1..=4 {
                instrument.state = hex_devices::operator_state::OPERATOR_METADATA.patch_parameter(
                    &instrument.state,
                    ParameterId::from_static(362 + (operator - 1) * 8),
                    ParameterValue::Choice(1),
                )?;
                instrument.state = hex_devices::operator_state::OPERATOR_METADATA.patch_parameter(
                    &instrument.state,
                    hex_devices::operator_state::parameter::envelope(
                        operator,
                        hex_devices::operator_state::parameter::EnvelopeField::Sustain,
                    ),
                    ParameterValue::Float(match profile {
                        CompanionProfile::Distinct => operator as f32 * 0.1,
                        CompanionProfile::Asymmetric if operator == 4 => 0.35,
                        _ => 0.2,
                    }),
                )?;
            }
            let receiver = source.receiver.ok_or("missing fixture source receiver")?;
            let track = snapshot
                .tracks
                .iter_mut()
                .find(|track| track.id == receiver)
                .ok_or("missing fixture receiver track")?;
            track
                .instrument
                .as_mut()
                .ok_or("missing fixture instrument projection")?
                .state = instrument.state.clone();
        }
    }
    let project = Project::from_snapshot(snapshot)?;
    let asset_id = project.assets().next().ok_or("missing fixture asset")?.id();
    let samples = (0..SOURCE)
        .map(|i| {
            let level = if i < SOURCE / 2 { 0.2 } else { 0.002 };
            let sign = if i % 32 < 16 { 1.0 } else { -1.0 };
            [level * sign, -level * sign * 0.65]
        })
        .collect();
    Ok((
        project,
        BTreeMap::from([(asset_id, Audio::new(RATE, samples)?)]),
    ))
}

pub(crate) fn revision(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    table: &Arc<SineTable>,
    previous: Option<&hex_engine::PreparedSessionInventory>,
) -> Result<Arc<PreparedRevision>> {
    Ok(Arc::new(PreparedRevision::prepare(
        project,
        media,
        Arc::clone(table),
        previous,
    )?))
}
pub(crate) fn playback(revision: Arc<PreparedRevision>) -> Result<SessionPlayback> {
    let mut result = SessionPlayback::new(revision)?;
    result.apply(SessionPlaybackCommand::Play)?;
    Ok(result)
}
pub(crate) fn digest(hash: &mut u64, frames: &[[f32; 2]]) {
    for sample in frames.iter().flatten() {
        *hash ^= u64::from(sample.to_bits());
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}
// Independent track composition: compile each original track as a separate
// project and add its entire output in the original mixer order. This exercises
// the same device implementations but not the shared 100-track mixing graph.
fn composition_oracle(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    table: &Arc<SineTable>,
    block: usize,
    total: usize,
) -> Result<Vec<[f32; 2]>> {
    let snapshot = project.snapshot();
    let mut sum = vec![[0.0_f32; 2]; total];
    let mut output = vec![[0.0_f32; 2]; block];
    for track in &snapshot.tracks {
        let mut isolated = snapshot.clone();
        isolated.tracks.retain(|candidate| candidate.id == track.id);
        isolated
            .native_sources
            .retain(|source| source.receiver == Some(track.id));
        let isolated = Project::from_snapshot(isolated)?;
        let prepared = revision(&isolated, media, table, None)?;
        let mut playback = playback(prepared)?;
        for (offset, chunk) in sum.chunks_mut(block).enumerate() {
            playback.render(&mut output[..chunk.len()])?;
            for (mixed, source) in chunk.iter_mut().zip(&output) {
                for channel in 0..2 {
                    if !source[channel].is_finite() {
                        return Err(format!(
                            "nonfinite isolated PCM at track {} block {offset}",
                            track.id
                        )
                        .into());
                    }
                    mixed[channel] += source[channel];
                }
            }
        }
    }
    Ok(sum)
}
// Audio sources finish at the handoff while held Operator notes continue.
// On bypass, Utility ramps the continuously filtered output to dry; removal
// retains the detached tail. Isolated tracks provide independent references.
fn mixed_tail(
    tracks: usize,
    block: usize,
    profile: CompanionProfile,
    detached: bool,
) -> Result<()> {
    let (base, mut media) = if profile != CompanionProfile::Default {
        fixture_with_companion(tracks, 1, profile)?
    } else {
        fixture(tracks, 1)?
    };
    let asset = base.assets().next().ok_or("missing mixed-tail asset")?.id();
    media.insert(asset, Audio::new(RATE, vec![[0.3, -0.1]; 512])?);
    let mut snapshot = base.snapshot();
    let dc = utility::UTILITY_DEFINITION.patch_parameter(
        &utility::UTILITY_DEFINITION.default_state()?,
        utility::parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    )?;
    for track in &mut snapshot.tracks {
        track
            .devices
            .first_mut()
            .ok_or("mixed-tail track lacks an effect")?
            .state = dc.clone();
        if let TrackContentSnapshot::Audio(clips)
        | TrackContentSnapshot::Hybrid {
            audio_clips: clips, ..
        } = &mut track.content
        {
            if clips.is_empty() {
                continue;
            }
            let clip = &mut clips[0];
            clip.duration = 32;
            clip.looping = false;
            clip.fade = ClipFade::neutral(32)?;
        }
    }
    let old = Project::from_snapshot(snapshot.clone())?;
    for track in &mut snapshot.tracks {
        if track.instrument.is_none() {
            if detached {
                track.devices.clear();
            } else {
                track
                    .devices
                    .first_mut()
                    .ok_or("mixed-tail track lacks an effect")?
                    .bypassed = true;
            }
        }
    }
    let changed = Project::from_snapshot(snapshot)?;
    let table = Arc::new(SineTable::new());
    let frames = RATE as usize + 32;
    let mut expected = composition_oracle(&old, &media, &table, 32, frames)?;
    // Independently demonstrate an audible audio-only residual after source end.
    let mut audio_only = old.snapshot();
    audio_only.tracks.retain(|track| track.instrument.is_none());
    audio_only.native_sources.clear();
    let audio_only = Project::from_snapshot(audio_only)?;
    let audio_reference = composition_oracle(&audio_only, &media, &table, 32, frames)?;
    let audio_peak = audio_reference[32..]
        .iter()
        .flatten()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    if audio_peak <= 2.0e-5 {
        return Err(format!("mixed-tail audio reference too quiet: {audio_peak:e}").into());
    }
    if !detached {
        // The dry audio is zero after the clips end at frame 32. Apply the
        // documented 5 ms Utility wet-to-dry law only to the isolated wet signal.
        // The instrument path is unchanged and keeps its full level.
        let ramp_frames = RATE.div_ceil(200) as usize;
        let mut ramp_peak = 0.0_f32;
        let mut first_stale_delta = None;
        for (offset, (mixed, wet)) in expected[32..]
            .iter_mut()
            .zip(&audio_reference[32..])
            .enumerate()
        {
            let dry_mix = (offset + 1).min(ramp_frames) as f32 / ramp_frames as f32;
            for lane in 0..2 {
                let stale = mixed[lane];
                mixed[lane] -= wet[lane] * dry_mix;
                let delta = (stale - mixed[lane]).abs();
                if first_stale_delta.is_none() && delta > 2.0e-6 {
                    first_stale_delta = Some((offset / block, 32 + offset, lane, delta));
                }
                ramp_peak = ramp_peak.max((wet[lane] * (1.0 - dry_mix)).abs());
            }
        }
        if ramp_peak <= 2.0e-5 {
            return Err(format!("mixed-tail bypass reference too quiet: {ramp_peak:e}").into());
        }
        if let Some((block, frame, lane, delta)) = first_stale_delta {
            println!(
                "unfaded_reference profile={profile:?} block={block} frame={frame} channel={lane} delta={delta:e}"
            );
        }
    }
    let mut instrument_only = old.snapshot();
    instrument_only
        .tracks
        .retain(|track| track.instrument.is_some());
    let instrument_only = Project::from_snapshot(instrument_only)?;
    let instrument_reference = composition_oracle(&instrument_only, &media, &table, 32, 1024)?;
    if !instrument_reference[32..]
        .iter()
        .flatten()
        .any(|value| value.abs() > 1.0e-6)
    {
        return Err("mixed-tail instrument reference is silent after source end".into());
    }
    let old_revision = revision(&old, &media, &table, None)?;
    let candidate_revision = revision(&changed, &media, &table, Some(old_revision.session()))?;
    let mut previous = playback(old_revision)?;
    let mut prefix = [[0.0_f32; 2]; 32];
    previous.render(&mut prefix)?;
    let mut candidate = SessionPlayback::new(candidate_revision)?;
    #[cfg(feature = "instrument-test-support")]
    let before = if detached {
        Some((
            previous
                .profile_runtime_storage_bytes()
                .ok_or("old storage overflow")?,
            candidate
                .profile_runtime_storage_bytes()
                .ok_or("candidate storage overflow")?,
        ))
    } else {
        None
    };
    let handoff_start = Instant::now();
    candidate.try_inherit_state_from(&mut previous)?;
    let handoff_ns = handoff_start.elapsed().as_nanos();
    let mut output = vec![[0.0_f32; 2]; block];
    let mut hash = 0xcbf29ce484222325;
    let mut max_error = 0.0_f32;
    let mut first_render_ns = 0;
    for (index, expected_chunk) in expected[32..].chunks(block).enumerate() {
        let count = expected_chunk.len();
        let start = Instant::now();
        candidate.render(&mut output[..count])?;
        let elapsed = start.elapsed().as_nanos();
        if index == 0 {
            first_render_ns = elapsed;
            #[cfg(feature = "instrument-test-support")]
            if let Some((old, before)) = before {
                let retired = previous
                    .profile_runtime_storage_bytes()
                    .ok_or("retired storage overflow")?;
                let after = candidate
                    .profile_runtime_storage_bytes()
                    .ok_or("candidate storage overflow")?;
                if old.0 == 0
                    || before.0 == 0
                    || retired.0 == 0
                    || old.1 != 0
                    || before.1 != 0
                    || after.1 == 0
                    || after.1 > after.0
                {
                    return Err("unexpected detached-tail custody".into());
                }
                println!(
                    "runtime_ledger_bytes old_before={} candidate_before={} retired_after_first_render={} candidate_after_first_render={} detached_track_tails_in_candidate={}; ledger charges, not unique memory or RSS; shared assets, native resources and allocator overhead excluded",
                    old.0, before.0, retired.0, after.0, after.1
                );
            }
        }
        for (offset, (actual, expected)) in output[..count].iter().zip(expected_chunk).enumerate() {
            for channel in 0..2 {
                let error = (actual[channel] - expected[channel]).abs();
                if !error.is_finite() || error > 2.0e-6 {
                    let frame = 32 + index * block + offset;
                    return Err(format!(
                        "mixed tail composition mismatch profile={profile:?} block={index} frame={frame} channel={channel} actual={} expected={} error={error}",
                        actual[channel], expected[channel]
                    )
                    .into());
                }
                max_error = max_error.max(error);
            }
        }
        digest(&mut hash, &output[..count]);
    }
    println!(
        "mixed_tail detached={detached} tracks={tracks} audio_tracks={} operator_tracks={} profile={profile:?} block={block} rate={RATE} post_cut_frames={} digest={hash:016x} composition_max_error={max_error:e} audio_tail_peak={audio_peak:e} handoff_us={:.3} first_render_us={:.3} combined_us={:.3}; separate preparation/retirement; one transition, no physical deadline",
        tracks.div_ceil(2),
        tracks / 2,
        RATE,
        handoff_ns as f64 / 1000.0,
        first_render_ns as f64 / 1000.0,
        (handoff_ns + first_render_ns) as f64 / 1000.0
    );
    Ok(())
}
fn run(
    tracks: usize,
    devices: usize,
    block: usize,
    seconds: usize,
    composition: bool,
    profile: CompanionProfile,
) -> Result<()> {
    let (project, media) = fixture_with_companion(tracks, devices, profile)?;
    let table = Arc::new(SineTable::new());
    let initial = match revision(&project, &media, &table, None) {
        Ok(value) => value,
        Err(error)
            if devices * tracks > 256
                && error.to_string() == "stateful device runtime exceeds its bounded budget" =>
        {
            println!(
                "tracks={tracks} devices_per_track={devices} profile={profile:?} requested_effects={} requested_mutable_instruments={} admission=refused error={error}",
                tracks * devices,
                tracks / 2
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let total = seconds * RATE as usize;
    let composed = if composition {
        Some(composition_oracle(&project, &media, &table, block, total)?)
    } else {
        None
    };
    let mut composition_max_error = 0.0_f32;
    let mut current_revision = Arc::clone(&initial);
    let mut actual = playback(Arc::clone(&initial))?;
    let mut reference = playback(initial)?;
    let mut a = vec![[0.0; 2]; block];
    let mut b = a.clone();
    let mut timings = Vec::new();
    let mut handoffs = Vec::new();
    let mut handoff_plus_render = Vec::new();
    let mut pending_handoff = None;
    let mut hash = 0xcbf29ce484222325;
    let mut frame = 0;
    let mut next_handoff = RATE as usize * 10;
    let mut peak = 0.0_f32;
    let mut window_peak = 0.0_f32;
    let mut first_audible_frame = None;
    let mut first_render_ns = 0;
    while frame < total {
        if frame >= next_handoff {
            let prepared = revision(&project, &media, &table, Some(current_revision.session()))?;
            let mut candidate = SessionPlayback::new(Arc::clone(&prepared))?;
            let start = Instant::now();
            candidate.try_inherit_state_from(&mut actual)?;
            let elapsed = start.elapsed().as_nanos();
            handoffs.push(elapsed);
            pending_handoff = Some(elapsed);
            // Old owners are retired outside the timed handoff and render.
            actual = candidate;
            current_revision = prepared;
            next_handoff += RATE as usize * 10;
        }
        let count = block.min(total - frame);
        let start = Instant::now();
        actual.render(black_box(&mut a[..count]))?;
        let elapsed = start.elapsed().as_nanos();
        if frame == 0 {
            first_render_ns = elapsed;
        }
        if let Some(handoff) = pending_handoff.take() {
            handoff_plus_render.push((count, handoff + elapsed));
        }
        timings.push((count, elapsed));
        reference.render(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Err(format!("continuous PCM mismatch at frame {frame}").into());
        }
        if let Some(composed) = &composed {
            for (actual, expected) in a[..count].iter().zip(&composed[frame..frame + count]) {
                for channel in 0..2 {
                    let error = (actual[channel] - expected[channel]).abs();
                    if !error.is_finite() || error > 2.0e-6 {
                        return Err(format!(
                            "isolated-track composition mismatch at frame {frame} error={error}"
                        )
                        .into());
                    }
                    composition_max_error = composition_max_error.max(error);
                }
            }
        }
        for (offset, channels) in a[..count].iter().enumerate() {
            for sample in channels {
                if !sample.is_finite() {
                    return Err("nonfinite output".into());
                }
                peak = peak.max(sample.abs());
                window_peak = window_peak.max(sample.abs());
                if first_audible_frame.is_none() && sample.abs() > 1.0e-6 {
                    first_audible_frame = Some(frame + offset);
                }
            }
            if (frame + offset + 1) % RATE as usize == 0 {
                if window_peak < 1.0e-6 {
                    return Err("silent one-second window".into());
                }
                window_peak = 0.0;
            }
        }
        digest(&mut hash, &a[..count]);
        frame += count;
    }
    if peak < 1.0e-6 {
        return Err("silent fixture".into());
    }
    let late = timings
        .iter()
        .filter(|&&(frames, ns)| ns * u128::from(RATE) > frames as u128 * 1_000_000_000)
        .count();
    let transition_late = handoff_plus_render
        .iter()
        .filter(|&&(frames, ns)| ns * u128::from(RATE) > frames as u128 * 1_000_000_000)
        .count();
    let transition_max = handoff_plus_render
        .iter()
        .map(|&(_, ns)| ns)
        .max()
        .unwrap_or(0);
    timings.sort_unstable_by_key(|&(_, ns)| ns);
    handoffs.sort_unstable();
    let p = |pct: usize| timings[(timings.len() - 1) * pct / 100].1 as f64 / 1000.0;
    println!(
        "tracks={tracks} audio_tracks={} operator_tracks={} devices_per_track={devices} effect_instances={} mutable_instruments={} total_stateful_instances={} profile={profile:?} admission=accepted rate={RATE} block={block} seconds={seconds} frames={frame} digest={hash:016x} peak={peak:.8} render_p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} beyond_actual_period={late} first_audible_frame={} first_render_us={:.3} handoffs={} handoff_max_us={:.3} handoff_plus_render_max_us={:.3} transition_beyond_period={transition_late} isolated_track_composition={composition} composition_max_error={composition_max_error:.9}",
        tracks.div_ceil(2),
        tracks / 2,
        tracks * devices,
        tracks / 2,
        tracks * devices + tracks / 2,
        p(50),
        p(95),
        p(99),
        p(100),
        first_audible_frame.ok_or("no audible frame")?,
        first_render_ns as f64 / 1000.0,
        handoffs.len(),
        handoffs.last().copied().unwrap_or(0) as f64 / 1000.0,
        transition_max as f64 / 1000.0
    );
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    const USAGE: &str = "usage: profile_stateful_mixed <devices:1|4|16> <block:128|512> <seconds:1..60> [--composition] [--companion|--asymmetric|--distinct], or --mixed-tail|--mixed-detached-tail <block:128|512> [--companion|--asymmetric|--distinct]";
    if args
        .first()
        .is_some_and(|arg| arg == "--mixed-tail" || arg == "--mixed-detached-tail")
    {
        let detached = args[0] == "--mixed-detached-tail";
        if detached && !cfg!(feature = "instrument-test-support") {
            return Err("mixed-detached-tail requires instrument-test-support".into());
        }
        let profile = match args.as_slice() {
            [_, _] => CompanionProfile::Default,
            [_, _, flag] => CompanionProfile::from_flag(flag).ok_or(USAGE)?,
            _ => return Err(USAGE.into()),
        };
        let block: usize = args[1].parse()?;
        if ![128, 512].contains(&block) {
            return Err("mixed-tail block must be 128 or 512".into());
        }
        return mixed_tail(TRACKS, block, profile, detached);
    }
    if args.len() < 3 || args.len() > 5 {
        return Err(USAGE.into());
    }
    let mut composition = false;
    let mut profile = CompanionProfile::Default;
    for flag in &args[3..] {
        if flag == "--composition" && !composition {
            composition = true;
        } else if profile == CompanionProfile::Default {
            profile = CompanionProfile::from_flag(flag).ok_or(USAGE)?;
        } else {
            return Err(USAGE.into());
        }
    }
    let devices = args[0].parse()?;
    let block = args[1].parse()?;
    let seconds = args[2].parse()?;
    if ![1, 4, 16].contains(&devices)
        || ![128, 512].contains(&block)
        || !(1..=60).contains(&seconds)
    {
        return Err("profile arguments out of bounds".into());
    }
    println!(
        "headless mixed resident audio and four-voice all-operator arrangement; identical revision handoffs; continuous playback PCM reference; uncontrolled host load; no physical callback or first-audible measurement"
    );
    run(TRACKS, devices, block, seconds, composition, profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Return only the digest so large inline playback owners never overlap
    // between profile comparisons on the test thread's normal stack.
    fn rendered_digest(
        project: &Project,
        media: &BTreeMap<hex_project::AssetId, Audio>,
        table: &Arc<SineTable>,
    ) -> Result<u64> {
        let mut runtime = playback(revision(project, media, table, None)?)?;
        let mut hash = 0xcbf29ce484222325;
        let mut block = [[0.0; 2]; 128];
        for _ in 0..375 {
            runtime.render(&mut block)?;
            digest(&mut hash, &block);
        }
        Ok(hash)
    }

    #[test]
    fn looping_companion_profiles_preserve_structure_and_change_pcm() -> Result<()> {
        let (looped, media) = fixture_with_companion(2, 1, CompanionProfile::Uniform)?;
        let (asymmetric, _) = fixture_with_companion(2, 1, CompanionProfile::Asymmetric)?;
        let (distinct, _) = fixture_with_companion(2, 1, CompanionProfile::Distinct)?;
        let (normal, _) = fixture(2, 1)?;
        let snapshot = looped.snapshot();
        let clips = match &snapshot.tracks[1].content {
            TrackContentSnapshot::Instrument(clips)
            | TrackContentSnapshot::Hybrid {
                midi_clips: clips, ..
            } => clips,
            TrackContentSnapshot::Audio(_) => panic!("expected MIDI fixture"),
        };
        assert_eq!(clips[0].length_raw, BeatTime::from_quarters(256)?.raw());
        let duration = BeatTime::from_quarters(240)?.raw();
        assert!(
            clips[0]
                .notes
                .iter()
                .all(|note| note.duration_raw == duration)
        );
        let source = looped
            .native_sources()
            .first()
            .ok_or("missing native source")?;
        for operator in 1..=4 {
            assert_eq!(
                hex_devices::operator_state::OPERATOR_METADATA.read_parameter(
                    source.instrument().state(),
                    ParameterId::from_static(362 + (operator - 1) * 8),
                )?,
                ParameterValue::Choice(1)
            );
        }
        let table = Arc::new(SineTable::new());
        let looped_hash = rendered_digest(&looped, &media, &table)?;
        let normal_hash = rendered_digest(&normal, &media, &table)?;
        let asymmetric_hash = rendered_digest(&asymmetric, &media, &table)?;
        let distinct_hash = rendered_digest(&distinct, &media, &table)?;
        assert_ne!(
            looped_hash, normal_hash,
            "companion mode must affect rendered PCM"
        );
        assert_ne!(
            looped_hash, asymmetric_hash,
            "asymmetry must affect rendered PCM"
        );
        assert_ne!(
            asymmetric_hash, distinct_hash,
            "distinct laws must affect rendered PCM"
        );
        Ok(())
    }

    #[test]
    fn uniform_companion_matches_continuous_and_composed_output() -> Result<()> {
        run(2, 1, 128, 1, true, CompanionProfile::Uniform)
    }

    #[test]
    fn asymmetric_companion_matches_continuous_and_composed_output() -> Result<()> {
        run(2, 1, 128, 1, true, CompanionProfile::Asymmetric)
    }

    #[test]
    fn distinct_companion_matches_continuous_and_composed_output() -> Result<()> {
        run(2, 1, 128, 1, true, CompanionProfile::Distinct)
    }

    #[test]
    fn mixed_fixture_handoff_preserves_continuous_pcm() -> Result<()> {
        let (project, media) = fixture(2, 1)?;
        let table = Arc::new(SineTable::new());
        let initial = revision(&project, &media, &table, None)?;
        let mut original = playback(Arc::clone(&initial))?;
        let mut reference = playback(Arc::clone(&initial))?;
        let mut a = [[0.0; 2]; 128];
        let mut b = a;
        for _ in 0..4 {
            original.render(&mut a)?;
            reference.render(&mut b)?;
            assert_eq!(a, b);
        }
        let prepared = revision(&project, &media, &table, Some(initial.session()))?;
        let mut candidate = SessionPlayback::new(prepared)?;
        candidate.try_inherit_state_from(&mut original)?;
        for _ in 0..4 {
            candidate.render(&mut a)?;
            reference.render(&mut b)?;
            assert_eq!(a, b);
        }
        assert!(a.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
        Ok(())
    }

    #[test]
    fn isolated_track_composition_matches_and_detects_missing_track() -> Result<()> {
        let (project, media) = fixture(2, 1)?;
        let table = Arc::new(SineTable::new());
        let composed = composition_oracle(&project, &media, &table, 128, 1024)?;
        let mut full = playback(revision(&project, &media, &table, None)?)?;
        let mut actual = [[0.0_f32; 2]; 1024];
        for chunk in actual.chunks_mut(128) {
            full.render(chunk)?;
        }
        assert!(
            actual
                .iter()
                .zip(&composed)
                .all(|(a, b)| (0..2).all(|c| (a[c] - b[c]).abs() <= 2.0e-6))
        );
        let mut missing = project.snapshot();
        missing.tracks.truncate(1);
        missing.native_sources.clear();
        let missing = Project::from_snapshot(missing)?;
        let incomplete = composition_oracle(&missing, &media, &table, 128, 1024)?;
        assert!(
            actual
                .iter()
                .zip(&incomplete)
                .any(|(a, b)| (0..2).any(|c| (a[c] - b[c]).abs() > 2.0e-6))
        );
        Ok(())
    }

    #[test]
    fn mixed_tail_keeps_audio_residual_and_sustained_instrument() -> Result<()> {
        mixed_tail(2, 128, CompanionProfile::Default, false)
    }

    #[cfg(feature = "instrument-test-support")]
    #[test]
    fn mixed_detached_tail_keeps_audio_instrument_and_accounted_custody() -> Result<()> {
        mixed_tail(2, 128, CompanionProfile::Default, true)
    }

    #[test]
    fn mixed_tail_keeps_audio_residual_and_sustained_instrument_uniform() -> Result<()> {
        mixed_tail(2, 128, CompanionProfile::Uniform, false)
    }

    #[test]
    fn mixed_tail_keeps_audio_residual_and_sustained_instrument_asymmetric() -> Result<()> {
        mixed_tail(2, 128, CompanionProfile::Asymmetric, false)
    }

    #[test]
    fn mixed_tail_keeps_audio_residual_and_sustained_instrument_distinct() -> Result<()> {
        mixed_tail(2, 128, CompanionProfile::Distinct, false)
    }

    #[test]
    fn dense_fixture_refuses_the_existing_effect_budget() -> Result<()> {
        let (project, media) = fixture(100, 4)?;
        let table = Arc::new(SineTable::new());
        let error = match revision(&project, &media, &table, None) {
            Ok(_) => return Err("400 stateful effects unexpectedly admitted".into()),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "stateful device runtime exceeds its bounded budget"
        );
        Ok(())
    }
}
