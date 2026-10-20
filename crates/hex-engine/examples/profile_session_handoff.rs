//! Canonical revision handoff benchmark; no device or UI runtime.
mod support;
use hex_engine::{PreparedRevision, SessionPlayback};
use hex_project::{Command, Editor};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
use support::session_fixture::{self, RATE};

const SAMPLES: usize = 100;
const BLOCK: usize = 64;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let loop_tempo = match arguments.next().as_deref() {
        None => false,
        Some("--loop-tempo") => true,
        Some(_) => return Err("usage: profile_session_handoff [--loop-tempo]".into()),
    };
    if arguments.next().is_some() {
        return Err("usage: profile_session_handoff [--loop-tempo]".into());
    }
    // Variable-tempo cuts follow a complete tempo cycle and enter the next
    // Session source occurrence, with both route classes holding notes.
    let initial_cut = if loop_tempo { 144_000 } else { 24_000 };
    let mut editor = Editor::new(session_fixture::project(loop_tempo)?, 8)?;
    let table = hex_engine::SharedSineTable::new_on_control();
    let media = BTreeMap::new();
    let mut revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &media,
        table.clone(),
        None,
    )?);
    let mut playback = session_fixture::kernel(Arc::clone(&revision))?;
    playback.render(&mut vec![[0.0; 2]; initial_cut])?;
    let track = editor
        .project()
        .tracks()
        .first()
        .ok_or("missing fixture track")?
        .id();
    let mut timings = Vec::with_capacity(SAMPLES);
    let mut checksum = 0_u64;
    for sample in 0..SAMPLES {
        // Control-side edit, graph preparation and candidate construction are
        // deliberately outside the handoff timer. Sources share COW identity.
        let (_, prepared) = editor.execute_prepared(
            Command::Gain {
                track,
                gain: if sample % 2 == 0 { 0.75 } else { 0.25 },
            },
            |project| -> Result<PreparedRevision, Box<dyn Error>> {
                Ok(PreparedRevision::prepare(
                    project,
                    &media,
                    table.clone(),
                    Some(revision.session()),
                )?)
            },
        )?;
        let prepared = Arc::new(prepared);
        let mut candidate = SessionPlayback::new(Arc::clone(&prepared))?;
        let cut = playback.performance_frame();
        let arrangement_cut = playback.arrangement_frame();
        let start = Instant::now();
        let report = candidate.try_inherit_state_from(black_box(&mut playback))?;
        timings.push(start.elapsed().as_nanos());
        black_box(report);
        if playback.performance_frame() != cut || playback.arrangement_frame() != arrangement_cut {
            return Err("handoff mutated the previous runtime".into());
        }
        if candidate.performance_frame() != cut || candidate.arrangement_frame() != arrangement_cut
        {
            return Err("handoff changed transport cursors".into());
        }
        let oracle = session_fixture::finite_oracle(editor.project(), loop_tempo)?;
        let mut expected = [[0.0; 2]; BLOCK];
        {
            let mut finite = hex_engine::Renderer::new(&oracle);
            finite.seek(cut);
            finite.play();
            finite.render(&mut expected)?;
        }
        let mut actual = [[0.0; 2]; BLOCK];
        candidate.render(&mut actual)?;
        if actual != expected {
            return Err("handoff PCM differs from independent finite compiler oracle".into());
        }
        for frame in black_box(actual) {
            checksum = checksum.wrapping_add(u64::from(frame[0].to_bits()));
            checksum = checksum.wrapping_add(u64::from(frame[1].to_bits()));
        }
        // Previous runtime/revision final drops occur here, outside timing.
        playback = candidate;
        revision = prepared;
    }
    timings.sort_unstable();
    println!(
        "canonical handoff: tracks=100 session_routes=50 arrangement_routes=50 rate={RATE} initial_cut={initial_cut} step_frames={BLOCK} gain_edits={SAMPLES} loop_tempo={loop_tempo}"
    );
    println!(
        "handoff_p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} checksum={checksum} performance_frame={} arrangement_frame={}",
        timings[SAMPLES / 2] as f64 / 1_000.0,
        timings[(SAMPLES * 95).div_ceil(100) - 1] as f64 / 1_000.0,
        timings[(SAMPLES * 99).div_ceil(100) - 1] as f64 / 1_000.0,
        timings[SAMPLES - 1] as f64 / 1_000.0,
        playback.performance_frame(),
        playback.arrangement_frame()
    );
    println!(
        "preparation, render/oracle comparison, checksums and drops outside handoff timing; gain edits only, no native callback, plugins, tempo/source edits or sustained deadline claim"
    );
    Ok(())
}
