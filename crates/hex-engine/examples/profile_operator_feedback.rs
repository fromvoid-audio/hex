//! Release profiler for registered Operator feedback rendering and live retargets.
//! Offline SessionPlayback only; this is not a CPAL deadline or Live comparison.
#[path = "support/operator_feedback_fixture.rs"]
mod fixture;

use fixture::Fixture;
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice::InstrumentHistoryRetargetPreparation;
use hex_project::{Command, Editor};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const DEFAULT_WARMUP_BLOCKS: usize = 100;
const DEFAULT_MEASURED_BLOCKS: usize = 2_000;
const CHECK_FRAMES: usize = 512;
const CHECK_PARTITION: usize = 128;
const FEEDBACK_A: f32 = 0.24;
const FEEDBACK_B: f32 = 0.63;
const LEVEL_A: f32 = 0.82;
const LEVEL_B: f32 = 0.57;

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Steady,
    Retarget,
    Both,
}

#[derive(Clone, Copy, Debug)]
struct Options {
    tracks: usize,
    notes: usize,
    block: usize,
    warmup: usize,
    measured: usize,
    scenario: Scenario,
    density_check: bool,
    all_operators: bool,
    shared_source: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    println!(
        "profile={} operator=registered feedback=nonzero states=feedback:{FEEDBACK_A}->{FEEDBACK_B},level:{LEVEL_A}->{LEVEL_B} ramp_default=5ms sample_rate={} tracks={} notes_per_track={} topology={} block_frames={} warmup_blocks={} measured_blocks={} scenario={:?} all_operators={} arch={} os={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        fixture::SAMPLE_RATE,
        options.tracks,
        options.notes,
        if options.shared_source {
            "shared_source"
        } else {
            "independent"
        },
        options.block,
        options.warmup,
        options.measured,
        options.scenario,
        options.all_operators,
        std::env::consts::ARCH,
        std::env::consts::OS,
    );

    let target_state = fixture::state_with_operators(FEEDBACK_A, LEVEL_A, options.all_operators)?;
    report_registered_storage(&target_state)?;

    let initial = if options.shared_source {
        fixture::shared_source_project_with_operators(
            options.tracks,
            options.notes,
            FEEDBACK_A,
            LEVEL_A,
            options.all_operators,
        )?
    } else {
        fixture::project_with_operators(
            options.tracks,
            options.notes,
            FEEDBACK_A,
            LEVEL_A,
            options.all_operators,
        )?
    };
    println!(
        "topology_sources={} source_contributors={} authored_notes={} modeled_peak_authored_voices={}",
        initial.source_count,
        initial.contributor_count,
        initial.authored_notes,
        initial.peak_authored_voices,
    );
    let table = Arc::new(SineTable::new());
    let initial_revision = Arc::new(prepare_revision(&initial.project, &table, None)?);
    verify_partition_checksum(Arc::clone(&initial_revision))?;

    if options.density_check {
        verify_density_refusal(Arc::clone(&table), options.all_operators)?;
    }

    match options.scenario {
        Scenario::Steady => run_steady(&initial, &table, options)?,
        Scenario::Retarget => run_retarget(&initial, &table, options)?,
        Scenario::Both => {
            run_steady(&initial, &table, options)?;
            run_retarget(&initial, &table, options)?;
        }
    }
    Ok(())
}

fn run_steady(
    initial: &Fixture,
    table: &Arc<SineTable>,
    options: Options,
) -> Result<(), Box<dyn Error>> {
    let setup_started = Instant::now();
    let revision = Arc::new(prepare_revision(&initial.project, table, None)?);
    let mut playback = SessionPlayback::new(Arc::clone(&revision))?;
    playback.apply(SessionPlaybackCommand::Play)?;
    let setup_nanos = setup_started.elapsed().as_nanos();
    let mut output = vec![[0.0_f32; 2]; options.block];
    for _ in 0..options.warmup {
        playback.render(black_box(&mut output))?;
    }

    let mut timings = Vec::with_capacity(options.measured);
    let mut checksum = FNV_OFFSET;
    for _ in 0..options.measured {
        let started = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(started.elapsed().as_nanos());
        update_checksum(&mut checksum, black_box(&output));
    }

    println!("scenario=steady setup_and_runtime_creation_ns={setup_nanos}");
    print_stats("steady_render", &timings, options.block, true);
    println!(
        "steady_checksum={checksum:016x} rendered_frames={} modeled_peak_authored_voices={} session_playback_host_bytes=not_exposed_by_public_api",
        options.block * options.measured,
        initial.peak_authored_voices,
    );
    Ok(())
}

fn run_retarget(
    initial: &Fixture,
    table: &Arc<SineTable>,
    options: Options,
) -> Result<(), Box<dyn Error>> {
    let initial_setup_started = Instant::now();
    let mut editor = Editor::new(initial.project.clone(), 8)?;
    let mut revision = Arc::new(prepare_revision(editor.project(), table, None)?);
    let mut playback = SessionPlayback::new(Arc::clone(&revision))?;
    playback.apply(SessionPlaybackCommand::Play)?;
    let mut output = vec![[0.0_f32; 2]; options.block];
    let mut current_variant = vec![false; initial.instrument_ids.len()];
    let mut setup_timings = Vec::with_capacity(options.measured);
    let mut handoff_timings = Vec::with_capacity(options.measured);
    let mut render_timings = Vec::with_capacity(options.measured);
    let mut checksum = FNV_OFFSET;
    let initial_setup_nanos = initial_setup_started.elapsed().as_nanos();

    let warmup_and_measured = options
        .warmup
        .checked_add(options.measured)
        .ok_or("retarget block count overflow after option validation")?;
    for iteration in 0..warmup_and_measured {
        let track_index = iteration % initial.instrument_ids.len();
        current_variant[track_index] = !current_variant[track_index];
        let (feedback, level) = if current_variant[track_index] {
            (FEEDBACK_B, LEVEL_B)
        } else {
            (FEEDBACK_A, LEVEL_A)
        };

        let setup_started = Instant::now();
        editor.execute(Command::ReplaceDeviceState {
            device: initial.instrument_ids[track_index],
            state: fixture::state_with_operators(feedback, level, options.all_operators)?,
        })?;
        let prepared = Arc::new(prepare_revision(
            editor.project(),
            table,
            Some(revision.session()),
        )?);
        let mut candidate = SessionPlayback::new(Arc::clone(&prepared))?;
        let setup_elapsed = setup_started.elapsed().as_nanos();

        let handoff_started = Instant::now();
        let handoff = candidate.try_inherit_state_from(&mut playback)?;
        black_box(handoff);
        let handoff_elapsed = handoff_started.elapsed().as_nanos();

        let render_started = Instant::now();
        candidate.render(black_box(&mut output))?;
        let render_elapsed = render_started.elapsed().as_nanos();
        if iteration >= options.warmup {
            update_checksum(&mut checksum, black_box(&output));
            setup_timings.push(setup_elapsed);
            handoff_timings.push(handoff_elapsed);
            render_timings.push(render_elapsed);
        }

        // Retire old graph owners on this control-side example loop, outside
        // every handoff and render timing interval.
        let retired_playback = std::mem::replace(&mut playback, candidate);
        let retired_revision = std::mem::replace(&mut revision, prepared);
        drop(retired_playback);
        drop(retired_revision);
    }

    println!(
        "scenario=supported_live_retarget initial_setup_and_runtime_creation_ns={initial_setup_nanos}"
    );
    print_stats(
        "retarget_candidate_edit_prepare_and_create",
        &setup_timings,
        options.block,
        false,
    );
    print_stats("retarget_handoff", &handoff_timings, options.block, false);
    print_stats(
        "retarget_following_render",
        &render_timings,
        options.block,
        true,
    );
    println!(
        "retarget_checksum={checksum:016x} measured_handoffs={} rendered_frames={} retargeted_track_per_handoff=1 modeled_peak_authored_voices={} session_playback_host_bytes=not_exposed_by_public_api",
        handoff_timings.len(),
        options.block * options.measured,
        initial.peak_authored_voices,
    );
    Ok(())
}

fn prepare_revision(
    project: &hex_project::Project,
    table: &Arc<SineTable>,
    previous_session: Option<&hex_engine::PreparedSessionInventory>,
) -> Result<PreparedRevision, Box<dyn Error>> {
    Ok(PreparedRevision::prepare(
        project,
        &BTreeMap::new(),
        Arc::clone(table),
        previous_session,
    )?)
}

fn report_registered_storage(
    state: &hex_lattice_contract::NativeDeviceState,
) -> Result<(), Box<dyn Error>> {
    let registry = hex_devices::native_instrument_registry()?;
    let prepared = registry.prepare_instrument(
        state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: fixture::SAMPLE_RATE,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        },
    )?;
    let factory = prepared
        .prepare_mutable_runtime()?
        .ok_or("nonzero feedback did not select the registered mutable runtime")?;
    let pair_bytes = factory
        .declared_twin_bytes()
        .checked_mul(2)
        .ok_or("declared Operator runtime pair size overflow")?;
    let proof = match factory.prepare_history_retarget_target()? {
        InstrumentHistoryRetargetPreparation::Prepared(proof) => proof,
        InstrumentHistoryRetargetPreparation::Unchanged
        | InstrumentHistoryRetargetPreparation::RequiresQuiescence => {
            return Err("registered Operator did not provide its retarget target proof".into());
        }
    };
    println!(
        "declared_registered_operator_runtime_twin_bytes={} declared_pair_bytes={} target_proof_bytes={} host_total_bytes=not_exposed_by_public_api",
        factory.declared_twin_bytes(),
        pair_bytes,
        proof.declared_fixed_storage_bytes(),
    );
    Ok(())
}

fn verify_partition_checksum(revision: Arc<PreparedRevision>) -> Result<(), Box<dyn Error>> {
    let mut whole = SessionPlayback::new(Arc::clone(&revision))?;
    let mut partitioned = SessionPlayback::new(revision)?;
    whole.apply(SessionPlaybackCommand::Play)?;
    partitioned.apply(SessionPlaybackCommand::Play)?;
    let mut expected = [[0.0_f32; 2]; CHECK_FRAMES];
    whole.render(&mut expected)?;
    let mut actual = [[0.0_f32; 2]; CHECK_FRAMES];
    for chunk in actual.chunks_exact_mut(CHECK_PARTITION) {
        partitioned.render(chunk)?;
    }
    ensure_finite_audible_pcm("whole-block partition", &expected)?;
    ensure_finite_audible_pcm("split-block partition", &actual)?;
    if expected != actual {
        return Err("registered Operator PCM changed across 128/512-frame partitions".into());
    }
    let mut checksum = FNV_OFFSET;
    update_checksum(&mut checksum, &expected);
    println!(
        "partition_pcm_check=pass partitions=1x{CHECK_FRAMES},4x{CHECK_PARTITION} checksum={checksum:016x}"
    );
    Ok(())
}

fn ensure_finite_audible_pcm(label: &str, frames: &[[f32; 2]]) -> Result<(), Box<dyn Error>> {
    let mut has_nonzero_sample = false;
    for sample in frames.iter().flatten() {
        if !sample.is_finite() {
            return Err(format!("{label} contains non-finite PCM").into());
        }
        has_nonzero_sample |= *sample != 0.0;
    }
    if !has_nonzero_sample {
        return Err(format!("{label} rendered silence").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_finite_audible_pcm;

    #[test]
    fn partition_fixture_guard_rejects_silence_and_non_finite_pcm() {
        assert!(ensure_finite_audible_pcm("silent fixture", &[[0.0_f32, 0.0]]).is_err());
        assert!(ensure_finite_audible_pcm("invalid fixture", &[[f32::NAN, 0.0]]).is_err());
        assert!(ensure_finite_audible_pcm("audible fixture", &[[0.0, 0.25]]).is_ok());
    }
}

fn verify_density_refusal(
    table: Arc<SineTable>,
    all_operators: bool,
) -> Result<(), Box<dyn Error>> {
    let overfull = if all_operators {
        fixture::project_with_operators(1, 17, FEEDBACK_A, LEVEL_A, true)?
    } else {
        fixture::project(1, 17, FEEDBACK_A, LEVEL_A)?
    };
    let revision = match prepare_revision(&overfull.project, &table, None) {
        Ok(revision) => Arc::new(revision),
        Err(error) if is_density_refusal(&error) => {
            println!(
                "density_refusal_check=pass phase=revision_preparation voices_per_track=17 limit=16"
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut playback = match SessionPlayback::new(revision) {
        Ok(playback) => playback,
        Err(error) if is_density_refusal(&error) => {
            println!(
                "density_refusal_check=pass phase=playback_setup voices_per_track=17 limit=16"
            );
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    playback.apply(SessionPlaybackCommand::Play)?;
    let mut frame = [[0.0_f32; 2]; 1];
    match playback.render(&mut frame) {
        Ok(()) => return Err("17 simultaneous Operator notes were not refused".into()),
        Err(error) if is_density_refusal(&error) => {}
        Err(error) => return Err(error.into()),
    }
    println!("density_refusal_check=pass phase=render voices_per_track=17 limit=16");
    Ok(())
}

fn is_density_refusal(error: &impl std::fmt::Display) -> bool {
    error
        .to_string()
        .contains("midi voice density exceeds the instrument capacity per track")
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn update_checksum(checksum: &mut u64, frames: &[[f32; 2]]) {
    for sample in frames.iter().flatten() {
        *checksum = (*checksum ^ u64::from(sample.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn print_stats(label: &str, samples: &[u128], block_frames: usize, compare_deadline: bool) {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let total: u128 = samples.iter().sum();
    let deadline_ns = (block_frames as f64 / f64::from(fixture::SAMPLE_RATE) * 1e9) as u128;
    print!(
        "{label}: n={} total_ns={} p50_ns={} p95_ns={} p99_ns={} max_ns={}",
        sorted.len(),
        total,
        percentile(&sorted, 50),
        percentile(&sorted, 95),
        percentile(&sorted, 99),
        sorted.last().copied().unwrap_or(0),
    );
    if compare_deadline {
        println!(
            " block_deadline_ns={} max_over_deadline={:.3}x",
            deadline_ns,
            sorted.last().copied().unwrap_or(0) as f64 / deadline_ns.max(1) as f64,
        );
    } else {
        println!();
    }
}

fn percentile(sorted: &[u128], percentile: usize) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        tracks: 1,
        notes: 1,
        block: 128,
        warmup: DEFAULT_WARMUP_BLOCKS,
        measured: DEFAULT_MEASURED_BLOCKS,
        scenario: Scenario::Both,
        density_check: false,
        all_operators: false,
        shared_source: false,
    };
    for argument in std::env::args().skip(1) {
        if argument == "--density-check" {
            options.density_check = true;
        } else if argument == "--all-operators" {
            options.all_operators = true;
        } else if argument == "--shared-source" {
            options.shared_source = true;
        } else if let Some(value) = argument.strip_prefix("--tracks=") {
            options.tracks = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--notes=") {
            options.notes = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--block=") {
            options.block = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--warmup-blocks=") {
            options.warmup = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--measured-blocks=") {
            options.measured = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--scenario=") {
            options.scenario = match value {
                "steady" => Scenario::Steady,
                "retarget" => Scenario::Retarget,
                "both" => Scenario::Both,
                _ => return Err("--scenario must be steady, retarget, or both".into()),
            };
        } else {
            return Err(usage().into());
        }
    }
    if !matches!(options.tracks, 1 | 2 | 100)
        || !matches!(options.notes, 1 | 16)
        || !matches!(options.block, 128 | 512)
        || options.measured == 0
        || options.measured > 100_000
        || options.warmup > 100_000
    {
        return Err(usage().into());
    }
    if options.shared_source && !matches!(options.scenario, Scenario::Steady) {
        return Err("--shared-source currently supports --scenario=steady only".into());
    }
    let run_blocks = options
        .warmup
        .checked_add(options.measured)
        .ok_or("warmup and measured block count overflow")?;
    let run_frames = run_blocks
        .checked_mul(options.block)
        .ok_or("warmup and measured frame count overflow")?;
    if run_frames > fixture::NOTE_HOLD_FRAMES {
        return Err(format!(
            "requested run is {run_frames} frames, beyond the {}-frame held-note gate; lower warmup or measured blocks",
            fixture::NOTE_HOLD_FRAMES
        )
        .into());
    }
    Ok(options)
}

fn usage() -> &'static str {
    "usage: profile_operator_feedback [--tracks=1|2|100] [--notes=1|16] [--block=128|512] [--warmup-blocks=n] [--measured-blocks=n] [--scenario=steady|retarget|both] [--density-check] [--all-operators] [--shared-source (steady only)]"
}
