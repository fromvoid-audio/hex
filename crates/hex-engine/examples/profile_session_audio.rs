//! Headless 100-track Session audio workload; no native device, UI or plugins.
#[path = "support/session_audio_fixture.rs"]
mod session_audio_fixture;

use hex_engine::{
    Audio, Frame, LaunchQuantization, PlaybackAction, PreparedRevision, SessionPlaybackCommand,
    SineTable, playback::Playback,
};
use hex_project::{AssetId, BeatTime, Command, Editor, Project, SceneId, Tempo};
use session_audio_fixture as fixture;
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};

const BLOCK: usize = 128;
const RUNS: usize = 4_000;
const CONTROL_SAMPLES: usize = 100;
const WARMUPS: usize = 100;
const COLD_WARMUPS: usize = 5;
const PERIOD_NANOS_NUMERATOR: u128 = BLOCK as u128 * 1_000_000_000;

fn main() -> Result<(), Box<dyn Error>> {
    let distinct_assets = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--distinct-assets" => true,
        _ => return Err("usage: profile_session_audio [--distinct-assets]".into()),
    };
    let workload = fixture::fixture(distinct_assets)?;
    let (unique_assets, unique_frames, unique_bytes) = fixture::unique_pcm_summary(&workload.media);
    let active_frames = usize::try_from(fixture::WINDOW_A.1 - fixture::WINDOW_A.0)?;
    let active_bytes = active_frames * std::mem::size_of::<Frame>();
    let unique_active_bytes = active_bytes * unique_assets;
    let logical_active_bytes = active_bytes * workload.tracks.len();
    let mode = if distinct_assets {
        "distinct"
    } else {
        "shared"
    };
    println!(
        "session audio workload: mode={mode} arch={} os={} host_metadata=external_run_metadata rate={} block={BLOCK} tracks={} session_audio_clips={} arrangement_companions={} stop_cells={} scenes=3 assets={} unique_pcm_assets={unique_assets} unique_converted_frames={unique_frames} pcm_bytes={unique_bytes} active_window_frames={active_frames} active_window_bytes_per_track={active_bytes} active_window_unique_pcm_bytes={unique_active_bytes} logical_active_window_bytes={logical_active_bytes} resident_limit_frames=8000000",
        std::env::consts::ARCH,
        std::env::consts::OS,
        fixture::RATE,
        workload.tracks.len(),
        workload.scene_a_clips.len() + workload.scene_b_clips.len(),
        workload.tracks.len(),
        workload.tracks.len(),
        workload.project.assets().count(),
    );
    if unique_frames > 8_000_000 {
        return Err("fixture exceeds the documented resident frame budget".into());
    }

    verify_fixture(&workload.project, &workload.media, workload.scene_a)?;
    verify_fixture(&workload.project, &workload.media, workload.scene_b)?;
    verify_stop_and_arrangement(&workload.project, &workload.media, workload.stop_scene)?;
    verify_source_and_tempo_handoff(
        &workload.project,
        &workload.media,
        workload.scene_a,
        workload.scene_a_clips[0],
    )?;

    let sine_table = Arc::new(SineTable::new());
    for _ in 0..COLD_WARMUPS {
        black_box(PreparedRevision::prepare(
            &workload.project,
            &workload.media,
            Arc::clone(&sine_table),
            None,
        )?);
    }
    let mut cold_prepare = Vec::with_capacity(CONTROL_SAMPLES);
    for _ in 0..CONTROL_SAMPLES {
        let started = Instant::now();
        let candidate = PreparedRevision::prepare(
            black_box(&workload.project),
            &workload.media,
            Arc::clone(&sine_table),
            None,
        )?;
        let elapsed = started.elapsed().as_nanos();
        black_box(&candidate);
        cold_prepare.push(elapsed);
        drop(candidate);
    }
    report_control("cold_prepared_revision", &mut cold_prepare);
    println!("cold_prepare_warmups={COLD_WARMUPS}");

    let revision = Arc::new(PreparedRevision::prepare(
        &workload.project,
        &workload.media,
        Arc::clone(&sine_table),
        None,
    )?);
    let revision = profile_control_edits(
        &workload.project,
        &workload.media,
        &sine_table,
        revision,
        workload.tracks[0],
        workload.scene_a_clips[0],
    )?;
    profile_route_commands(Arc::clone(&revision), &workload)?;
    profile_active_render(
        Arc::clone(&revision),
        workload.scene_a,
        unique_active_bytes,
        logical_active_bytes,
        unique_bytes,
    )?;
    profile_exhausted_render(Arc::clone(&revision), workload.scene_a, active_frames)?;
    println!(
        "timers exclude fixture construction, PCM generation, independent oracle comparisons, playback construction, render launch/retrigger actions, checksum work, and final graph/PCM drops; action timings are direct headless Playback API costs, not callback queue/publication; offline only, no device/UI/plugin claims"
    );
    Ok(())
}

fn prepare_edit(
    editor: &mut Editor,
    command: Command,
    media: &BTreeMap<AssetId, Audio>,
    sine_table: &Arc<SineTable>,
    previous: &Arc<PreparedRevision>,
) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let previous = Arc::clone(previous);
    let (_, candidate) = editor.execute_prepared(command, |project| {
        Ok::<_, Box<dyn Error>>(PreparedRevision::prepare(
            project,
            media,
            Arc::clone(sine_table),
            Some(previous.session()),
        )?)
    })?;
    Ok(Arc::new(candidate))
}

fn retained_audio_pcm(before: &PreparedRevision, after: &PreparedRevision) -> usize {
    after
        .session()
        .audio_clips()
        .iter()
        .filter(|clip| {
            before.session().audio_clip(clip.id).is_some_and(|old| {
                old.resident_audio().map(|audio| audio.frames().as_ptr())
                    == clip.resident_audio().map(|audio| audio.frames().as_ptr())
            })
        })
        .count()
}

fn profile_control_edits(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    sine_table: &Arc<SineTable>,
    mut revision: Arc<PreparedRevision>,
    track: hex_project::TrackId,
    source_clip: hex_project::ClipId,
) -> Result<Arc<PreparedRevision>, Box<dyn Error>> {
    let mut editor = Editor::new(project.clone(), 8)?;
    let mut gain_times = Vec::with_capacity(CONTROL_SAMPLES);
    for sample in 0..CONTROL_SAMPLES {
        let next_gain = if sample % 2 == 0 { 0.125 } else { 0.0625 };
        let previous = Arc::clone(&revision);
        let started = Instant::now();
        let candidate = prepare_edit(
            &mut editor,
            Command::Gain {
                track,
                gain: next_gain,
            },
            media,
            sine_table,
            &previous,
        )?;
        gain_times.push(started.elapsed().as_nanos());
        let retained = retained_audio_pcm(&previous, &candidate);
        if retained != 200 {
            return Err(format!("gain edit retained PCM for {retained}/200 Session clips").into());
        }
        drop(previous);
        revision = candidate;
    }
    report_control("gain_edit_full_prepared_revision", &mut gain_times);
    println!("gain_edit_retained_pcm=200/200 (identity verification outside timer)");

    let asset = editor
        .project()
        .session_audio_clip(source_clip)
        .ok_or("missing source clip for edit timing")?
        .asset();
    let mut source_times = Vec::with_capacity(CONTROL_SAMPLES);
    for sample in 0..CONTROL_SAMPLES {
        let start = if sample % 2 == 0 {
            fixture::WINDOW_A.0 + 1
        } else {
            fixture::WINDOW_A.0
        };
        let previous = Arc::clone(&revision);
        let started = Instant::now();
        let candidate = prepare_edit(
            &mut editor,
            Command::SetSessionAudioClipSource {
                clip: source_clip,
                asset,
                source_start: start,
                source_end: fixture::WINDOW_A.1,
            },
            media,
            sine_table,
            &previous,
        )?;
        source_times.push(started.elapsed().as_nanos());
        let retained = retained_audio_pcm(&previous, &candidate);
        if retained != 200 {
            return Err(format!(
                "source-window edit retained PCM for {retained}/200 Session clips"
            )
            .into());
        }
        drop(previous);
        revision = candidate;
    }
    report_control(
        "source_window_edit_full_prepared_revision",
        &mut source_times,
    );
    println!("source_window_edit_retained_pcm=200/200 (identity verification outside timer)");
    Ok(revision)
}

fn profile_route_commands(
    revision: Arc<PreparedRevision>,
    workload: &fixture::Fixture,
) -> Result<(), Box<dyn Error>> {
    let project = &workload.project;
    let media = &workload.media;
    let scene_a = workload.scene_a;
    let scene_b = workload.scene_b;
    let stop_scene = workload.stop_scene;
    let clip_a = workload.scene_a_clips[0];
    let clip_b = workload.scene_b_clips[0];
    let mut playback = launch_scene(Arc::clone(&revision), scene_a)?;
    let mut scene_times = Vec::with_capacity(CONTROL_SAMPLES);
    for index in 0..CONTROL_SAMPLES {
        advance_outside_timer(&mut playback, 13 + index % 7)?;
        let scene = if index % 2 == 0 { scene_a } else { scene_b };
        let started = Instant::now();
        action(
            &mut playback,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
        scene_times.push(started.elapsed().as_nanos());
    }
    report_control("session_scene_launch_and_play_action", &mut scene_times);

    let mut playback = launch_scene(Arc::clone(&revision), scene_a)?;
    let mut switch_times = Vec::with_capacity(CONTROL_SAMPLES);
    for index in 0..CONTROL_SAMPLES {
        let clip = if index % 2 == 0 { clip_b } else { clip_a };
        advance_outside_timer(&mut playback, 17 + index % 11)?;
        let elapsed = playback.performance_frame();
        let started = Instant::now();
        action(
            &mut playback,
            PlaybackAction::Session(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
        switch_times.push(started.elapsed().as_nanos());
        verify_target_clip_render(&mut playback, project, media, scene_a, clip, elapsed)?;
    }
    report_control("session_clip_switch_action", &mut switch_times);

    let mut playback = launch_scene(Arc::clone(&revision), scene_a)?;
    let mut retrigger_times = Vec::with_capacity(CONTROL_SAMPLES);
    for index in 0..CONTROL_SAMPLES {
        advance_outside_timer(&mut playback, 19 + index % 13)?;
        let elapsed = playback.performance_frame();
        if index == 0 {
            verify_retrigger_contrast(project, media, clip_a, elapsed)?;
        }
        let started = Instant::now();
        action(
            &mut playback,
            PlaybackAction::Session(SessionPlaybackCommand::LaunchClip {
                clip: clip_a,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
        retrigger_times.push(started.elapsed().as_nanos());
        verify_target_clip_render(&mut playback, project, media, scene_a, clip_a, elapsed)?;
    }
    report_control("same_clip_retrigger_action", &mut retrigger_times);
    println!(
        "clip switch and same-clip retrigger each advanced source elapsed before every timed action; post-action PCM was checked against a direct source-frame oracle outside timers"
    );

    let mut playback = launch_scene(Arc::clone(&revision), scene_a)?;
    let mut stop_times = Vec::with_capacity(CONTROL_SAMPLES);
    for _ in 0..CONTROL_SAMPLES {
        advance_outside_timer(&mut playback, 23)?;
        let started = Instant::now();
        action(
            &mut playback,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                scene: stop_scene,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
        stop_times.push(started.elapsed().as_nanos());
        action(
            &mut playback,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                scene: scene_a,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
    }
    report_control("stop_scene_launch_action", &mut stop_times);

    let mut playback = launch_scene(Arc::clone(&revision), scene_a)?;
    let mut return_times = Vec::with_capacity(CONTROL_SAMPLES);
    for _ in 0..CONTROL_SAMPLES {
        advance_outside_timer(&mut playback, 29)?;
        let started = Instant::now();
        action(
            &mut playback,
            PlaybackAction::Session(SessionPlaybackCommand::BackToArrangement {
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
        return_times.push(started.elapsed().as_nanos());
        action(
            &mut playback,
            PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                scene: scene_a,
                quantization: LaunchQuantization::Immediate,
            }),
        )?;
    }
    report_control("back_to_arrangement_action", &mut return_times);
    println!(
        "route command timings use direct PlaybackAction calls on prepared offline runtimes with nonzero advancing anchors; action setup, pre-action render, post-action oracle, and relaunch are outside timers; no callback ring/publication claims"
    );
    Ok(())
}

fn advance_outside_timer(playback: &mut Playback, frames: usize) -> Result<(), Box<dyn Error>> {
    let mut output = [[0.0; 2]; 32];
    playback.render(&mut output[..frames])?;
    Ok(())
}

fn verify_target_clip_render(
    playback: &mut Playback,
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    continuing_scene: SceneId,
    target_clip_id: hex_project::ClipId,
    continuing_elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let partitions = [1, 7];
    let actual = fixture::render_partitions(playback, &partitions)?;
    let target = project
        .session_audio_clip(target_clip_id)
        .ok_or("missing target Session audio clip")?;
    let target_track = project
        .session_cells()
        .iter()
        .find_map(|cell| {
            matches!(cell.content(), hex_project::SessionCellContent::Audio(clip) if clip.id() == target_clip_id)
                .then_some(cell.track())
        })
        .ok_or("missing target Session audio cell")?;
    let mut expected = vec![[0.0; 2]; partitions.iter().sum()];
    for (offset, mixed) in expected.iter_mut().enumerate() {
        for track in project.tracks() {
            if track.muted() {
                continue;
            }
            let source = if track.id() == target_track {
                let frame = target
                    .source_start()
                    .checked_add(u64::try_from(offset)?)
                    .ok_or("retrigger source frame overflow")?;
                if frame >= target.source_end() {
                    None
                } else {
                    Some((target.asset(), frame))
                }
            } else {
                match project.session_cell(track.id(), continuing_scene) {
                    Some(hex_project::SessionCellContent::Audio(clip)) => {
                        let frame = clip
                            .source_start()
                            .checked_add(continuing_elapsed)
                            .and_then(|frame| frame.checked_add(u64::try_from(offset).ok()?))
                            .ok_or("continuing source frame overflow")?;
                        (frame < clip.source_end()).then_some((clip.asset(), frame))
                    }
                    _ => None,
                }
            };
            let Some((asset, frame)) = source else {
                continue;
            };
            let sample = *media
                .get(&asset)
                .ok_or("missing direct-oracle PCM asset")?
                .frames()
                .get(usize::try_from(frame)?)
                .ok_or("direct-oracle source outside PCM")?;
            mixed[0] += sample[0] * track.gain() * (1.0 - track.balance().max(0.0));
            mixed[1] += sample[1] * track.gain() * (1.0 + track.balance().min(0.0));
        }
        mixed[0] *= project.master_gain();
        mixed[1] *= project.master_gain();
    }
    if actual != expected {
        return Err(format!(
            "Session clip launch PCM mismatch for clip {} at performance frame {continuing_elapsed}",
            target_clip_id.value()
        )
        .into());
    }
    Ok(())
}

fn verify_retrigger_contrast(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    clip_id: hex_project::ClipId,
    elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let clip = project
        .session_audio_clip(clip_id)
        .ok_or("missing retrigger source clip")?;
    let continued = clip
        .source_start()
        .checked_add(elapsed)
        .ok_or("retrigger continuation frame overflow")?;
    if continued >= clip.source_end() {
        return Err("retrigger test advanced beyond finite clip source".into());
    }
    let audio = media
        .get(&clip.asset())
        .ok_or("missing retrigger PCM asset")?;
    let restart = *audio
        .frames()
        .get(usize::try_from(clip.source_start())?)
        .ok_or("retrigger start is outside PCM")?;
    let continued = *audio
        .frames()
        .get(usize::try_from(continued)?)
        .ok_or("retrigger continuation is outside PCM")?;
    if restart == continued {
        return Err("fixture does not distinguish retrigger from source continuation".into());
    }
    Ok(())
}

fn verify_source_and_tempo_handoff(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    scene: SceneId,
    clip: hex_project::ClipId,
) -> Result<(), Box<dyn Error>> {
    let table = Arc::new(SineTable::new());
    let initial = Arc::new(PreparedRevision::prepare(
        project,
        media,
        Arc::clone(&table),
        None,
    )?);
    let mut previous = launch_scene(Arc::clone(&initial), scene)?;
    let prefix_parts = [127, 129, 257];
    let prefix = fixture::render_partitions(&mut previous, &prefix_parts)?;
    let prefix_oracle =
        fixture::finite_session_oracle(project, media, scene, 0, prefix_parts.iter().sum())?;
    if prefix != prefix_oracle {
        return Err("pre-handoff audio prefix differs from direct frame oracle".into());
    }
    let cut = previous.performance_frame();
    let mut editor = Editor::new(project.clone(), 8)?;
    let source = editor
        .project()
        .session_audio_clip(clip)
        .ok_or("missing one-shot source for handoff")?;
    let source_edit = Command::SetSessionAudioClipSource {
        clip,
        asset: source.asset(),
        source_start: source.source_start() + 256,
        source_end: source.source_end(),
    };
    let source_revision = prepare_edit(&mut editor, source_edit, media, &table, &initial)?;
    let mut source_candidate =
        Playback::try_from_prepared_with_session(Arc::clone(&source_revision))?;
    source_candidate
        .try_inherit_runtime_from(&previous)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;

    let tempo_revision = prepare_edit(
        &mut editor,
        Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(60_000_000)?,
        },
        media,
        &table,
        &source_revision,
    )?;
    let mut tempo_candidate =
        Playback::try_from_prepared_with_session(Arc::clone(&tempo_revision))?;
    tempo_candidate
        .try_inherit_runtime_from(&source_candidate)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    if tempo_candidate.performance_frame() != cut {
        return Err("tempo/source handoff changed the performance-frame cut".into());
    }
    let partitions = [1, 63, 128, 65];
    let actual = fixture::render_partitions(&mut tempo_candidate, &partitions)?;
    let expected = fixture::finite_session_oracle(
        editor.project(),
        media,
        scene,
        cut,
        partitions.iter().sum(),
    )?;
    if actual != expected {
        return Err("source-window and tempo handoff differs from direct frame oracle".into());
    }
    println!(
        "source+tempo handoff oracle passed: stable_clip={} cut_performance_frame={cut} next_frames={}",
        clip.value(),
        partitions.iter().sum::<usize>()
    );
    Ok(())
}

fn action(playback: &mut Playback, action: PlaybackAction) -> Result<(), Box<dyn Error>> {
    playback
        .try_apply_action(action)
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    Ok(())
}

fn launch_scene(
    revision: Arc<PreparedRevision>,
    scene: SceneId,
) -> Result<Playback, Box<dyn Error>> {
    let mut playback = Playback::try_from_prepared_with_session(revision)?;
    action(
        &mut playback,
        PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        }),
    )?;
    Ok(playback)
}

fn verify_fixture(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    scene: SceneId,
) -> Result<(), Box<dyn Error>> {
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(PreparedRevision::prepare(project, media, table, None)?);
    let partitions = [1, 31, 127, 128, 129, 255, 256, 257, 1024, 2045, 257];
    let frames = partitions.iter().sum::<usize>();
    let mut playback = launch_scene(Arc::clone(&revision), scene)?;
    let actual = fixture::render_partitions(&mut playback, &partitions)?;
    let expected = fixture::finite_session_oracle(project, media, scene, 0, frames)?;
    if actual != expected {
        return Err(format!("Session audio oracle mismatch for scene {}", scene.value()).into());
    }

    let mut long_playback = launch_scene(revision, scene)?;
    let end_probe = [4096, 257];
    let actual = fixture::render_partitions(&mut long_playback, &end_probe)?;
    let expected =
        fixture::finite_session_oracle(project, media, scene, 0, end_probe.iter().sum())?;
    if actual != expected {
        return Err("Session audio one-shot end/partition oracle mismatch".into());
    }
    let arrangement = fixture::finite_arrangement_oracle(project, media, 4096, 257)?;
    if arrangement.iter().all(|frame| *frame == [0.0; 2]) {
        return Err("Arrangement companions must distinguish route hold from fallback".into());
    }
    if actual[4096..].iter().any(|frame| *frame != [0.0; 2]) {
        return Err("exhausted one-shot must emit exact silence".into());
    }
    Ok(())
}

fn verify_stop_and_arrangement(
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    stop_scene: SceneId,
) -> Result<(), Box<dyn Error>> {
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(PreparedRevision::prepare(project, media, table, None)?);
    let mut playback = launch_scene(Arc::clone(&revision), stop_scene)?;
    let stopped = fixture::render_partitions(&mut playback, &[127, 129])?;
    if stopped.iter().any(|frame| *frame != [0.0; 2]) {
        return Err("Session Stop scene must silence every selected track".into());
    }
    let actual_arrangement_start = 256;
    action(
        &mut playback,
        PlaybackAction::Session(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        }),
    )?;
    let partitions = [1, 31, 64, 127, 129];
    let actual = fixture::render_partitions(&mut playback, &partitions)?;
    let expected = fixture::finite_arrangement_oracle(
        project,
        media,
        actual_arrangement_start,
        partitions.iter().sum(),
    )?;
    if actual != expected {
        return Err("Back to Arrangement companion PCM oracle mismatch".into());
    }
    Ok(())
}

fn profile_active_render(
    revision: Arc<PreparedRevision>,
    scene: SceneId,
    unique_active_bytes: usize,
    logical_active_bytes: usize,
    resident_pcm_bytes: usize,
) -> Result<(), Box<dyn Error>> {
    let mut playback = launch_scene(revision, scene)?;
    let mut output = [[0.0; 2]; BLOCK];
    let mut timings = Vec::with_capacity(RUNS);
    let mut checksum = 0_u64;
    let source_frames = usize::try_from(fixture::WINDOW_A.1 - fixture::WINDOW_A.0)?;
    let mut frames_since_launch = 0;
    for _ in 0..WARMUPS {
        if frames_since_launch == source_frames {
            action(
                &mut playback,
                PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                }),
            )?;
            frames_since_launch = 0;
        }
        playback.render(&mut output)?;
        frames_since_launch += BLOCK;
    }
    for _ in 0..RUNS {
        if frames_since_launch == source_frames {
            action(
                &mut playback,
                PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                }),
            )?;
            frames_since_launch = 0;
        }
        let started = Instant::now();
        playback.render(black_box(&mut output))?;
        let elapsed = started.elapsed().as_nanos();
        timings.push(elapsed);
        for frame in black_box(output) {
            checksum = checksum.wrapping_add(u64::from(frame[0].to_bits()));
            checksum = checksum.wrapping_add(u64::from(frame[1].to_bits()));
        }
        frames_since_launch += BLOCK;
    }
    report_render("active_session_render", &mut timings, checksum);
    println!(
        "active render retriggers only after each full {source_frames}-frame source window, outside render timer; every timed block starts with all 100 one-shots active; working_set_unique_pcm_bytes={} logical_working_set_bytes={} resident_pcm_bytes={}",
        unique_active_bytes, logical_active_bytes, resident_pcm_bytes
    );
    Ok(())
}

fn profile_exhausted_render(
    revision: Arc<PreparedRevision>,
    scene: SceneId,
    source_frames: usize,
) -> Result<(), Box<dyn Error>> {
    let mut playback = launch_scene(revision, scene)?;
    let _ = fixture::render_partitions(&mut playback, &[source_frames])?;
    let mut output = [[0.0; 2]; BLOCK];
    for _ in 0..WARMUPS {
        playback.render(&mut output)?;
    }
    if output.iter().any(|frame| *frame != [0.0; 2]) {
        return Err("exhausted Session audio should retain ownership and emit silence".into());
    }
    let mut timings = Vec::with_capacity(RUNS);
    let mut checksum = 0_u64;
    for _ in 0..RUNS {
        let started = Instant::now();
        playback.render(black_box(&mut output))?;
        timings.push(started.elapsed().as_nanos());
        for frame in black_box(output) {
            checksum = checksum.wrapping_add(u64::from(frame[0].to_bits()));
            checksum = checksum.wrapping_add(u64::from(frame[1].to_bits()));
        }
    }
    report_render("exhausted_session_render", &mut timings, checksum);
    Ok(())
}

fn report_control(name: &str, times: &mut [u128]) {
    times.sort_unstable();
    println!(
        "{name}: samples={} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        times.len(),
        nearest_rank(times, 50) as f64 / 1_000.0,
        nearest_rank(times, 95) as f64 / 1_000.0,
        nearest_rank(times, 99) as f64 / 1_000.0,
        times[times.len() - 1] as f64 / 1_000.0
    );
}

fn report_render(name: &str, times: &mut [u128], checksum: u64) {
    times.sort_unstable();
    let exceeded = times
        .iter()
        .filter(|&&nanos| nanos * u128::from(fixture::RATE) > PERIOD_NANOS_NUMERATOR)
        .count();
    println!(
        "{name}: block={BLOCK} samples={} warmups={WARMUPS} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} nominal_period_exceed={exceeded}/{} checksum={checksum}",
        times.len(),
        nearest_rank(times, 50) as f64 / 1_000.0,
        nearest_rank(times, 95) as f64 / 1_000.0,
        nearest_rank(times, 99) as f64 / 1_000.0,
        times[times.len() - 1] as f64 / 1_000.0,
        times.len()
    );
}

fn nearest_rank(times: &[u128], percentile: usize) -> u128 {
    let rank = (times.len() * percentile).div_ceil(100).max(1);
    times[rank - 1]
}
