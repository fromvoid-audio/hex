use super::*;
use hex_project::{Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn zero(measured: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current,
        ),
        (0, 0, 0, 0)
    );
}

fn assert_smooth_steps(samples: &[Frame], prev: Frame, max_step: f32) {
    let mut last = prev;
    for (index, &frame) in samples.iter().enumerate() {
        for ch in 0..2 {
            let delta = (frame[ch] - last[ch]).abs();
            assert!(
                delta <= max_step,
                "step jump at frame {index} ch {ch}: {} -> {} (delta {delta} > {max_step})",
                last[ch],
                frame[ch]
            );
        }
        last = frame;
    }
}

#[test]
fn resident_live_mute_unmute_and_solo_fade_smoothly_with_zero_allocations() {
    let _serial = serial();
    const RATE: u32 = 48_000;
    const FADE: usize = 240;
    const LEN: usize = 8192;
    const SIG_A: Frame = [0.4, -0.2];
    const SIG_B: Frame = [0.2, 0.3];
    const SUM: Frame = [SIG_A[0] + SIG_B[0], SIG_A[1] + SIG_B[1]];
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Imported {
        track: track_a,
        asset: asset_a,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "a".into(),
            path: "a.wav".into(),
            sample_rate: RATE,
            frames: LEN as u64,
        })
        .unwrap()
    else {
        panic!("track a");
    };
    let Created::Imported {
        track: track_b,
        asset: asset_b,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "b".into(),
            path: "b.wav".into(),
            sample_rate: RATE,
            frames: LEN as u64,
        })
        .unwrap()
    else {
        panic!("track b");
    };
    let media = BTreeMap::from([
        (
            asset_a,
            hex_engine::Audio::new(RATE, vec![SIG_A; LEN]).unwrap(),
        ),
        (
            asset_b,
            hex_engine::Audio::new(RATE, vec![SIG_B; LEN]).unwrap(),
        ),
    ]);
    let table = Arc::new(hex_engine::SineTable::new());
    let build = |project: &Project| {
        let rev = Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        );
        Playback::try_from_prepared_with_session(rev).unwrap()
    };
    let both = editor.project().clone();
    editor
        .execute(Command::Mute {
            track: track_b,
            muted: true,
        })
        .unwrap();
    let b_muted = editor.project().clone();
    editor
        .execute(Command::Mute {
            track: track_b,
            muted: false,
        })
        .unwrap();
    editor
        .execute(Command::ToggleSolo {
            track: track_a,
            exclusive: true,
        })
        .unwrap();
    let a_solo = editor.project().clone();

    let (mut control, mut callback) = channel(RATE).unwrap();
    let r1 = control
        .publish_with_policy(build(&both), PublicationPolicy::InheritRuntime)
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    zero(allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut block).unwrap();
    }));
    assert_eq!(control.status.applied_revision.load(Acquire), r1);
    assert_eq!(block[63], SUM);

    // 1. Live mute track B -> callback holds predecessor through smooth fade-out.
    let r2 = control
        .publish_with_policy(build(&b_muted), PublicationPolicy::InheritRuntime)
        .unwrap();
    let mut mute_pcm = Vec::new();
    while control.status.applied_revision.load(Acquire) != r2 {
        zero(allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        }));
        mute_pcm.extend_from_slice(&block);
        assert!(mute_pcm.len() <= FADE + 256, "mute fade did not commit");
    }
    assert!(mute_pcm.len() >= FADE);
    assert_smooth_steps(&mute_pcm, SUM, 0.3 / FADE as f32 + 1e-4);
    assert!((mute_pcm.last().unwrap()[0] - SIG_A[0]).abs() < 1e-5);
    assert!((mute_pcm.last().unwrap()[1] - SIG_A[1]).abs() < 1e-5);
    control.reclaim();

    // 2. Live unmute track B -> ramps smoothly back in.
    let r3 = control
        .publish_with_policy(build(&both), PublicationPolicy::InheritRuntime)
        .unwrap();
    let mut unmute_pcm = Vec::new();
    for _ in 0..6 {
        zero(allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        }));
        unmute_pcm.extend_from_slice(&block);
    }
    assert_eq!(control.status.applied_revision.load(Acquire), r3);
    assert_smooth_steps(&unmute_pcm, SIG_A, 0.3 / FADE as f32 + 1e-4);
    assert!((unmute_pcm.last().unwrap()[0] - SUM[0]).abs() < 1e-5);
    control.reclaim();

    // 3. Live solo track A -> track B fades out smoothly while A remains constant.
    let r4 = control
        .publish_with_policy(build(&a_solo), PublicationPolicy::InheritRuntime)
        .unwrap();
    let mut solo_pcm = Vec::new();
    while control.status.applied_revision.load(Acquire) != r4 {
        zero(allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        }));
        solo_pcm.extend_from_slice(&block);
        assert!(solo_pcm.len() <= FADE + 256, "solo fade did not commit");
    }
    assert!(solo_pcm.len() >= FADE);
    assert_smooth_steps(&solo_pcm, SUM, 0.3 / FADE as f32 + 1e-4);
    assert!((solo_pcm.last().unwrap()[0] - SIG_A[0]).abs() < 1e-5);
    control.reclaim();
}

#[test]
fn streamed_live_mute_and_unmute_fade_smoothly_with_zero_allocations() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    const FADE: usize = 240;
    let project = readiness::project_with_frames(12_288);
    let track = project.tracks()[0].id();
    let asset = project.assets().next().unwrap().id();
    let mut editor = Editor::new(project.clone(), 8).unwrap();
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    let muted_project = editor.project().clone();

    let wav = stream_fixture::Wav::with_frames(12_288);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let make_owner = |proj: &Project, editor_revision: u64| {
        stream_fixture::owner_with_context(
            proj,
            metadata.clone(),
            Some(hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision,
            }),
        )
    };
    let mut owner1 = make_owner(&project, 1);
    let limits = reservation(&mut owner1);
    let (control, callback) = channel(48_000).unwrap();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None);
    let mut retirement =
        crate::native_callback_owner::NativeRetirement::new(terminals, root.clone());
    let control = publication.control().unwrap();
    control.enable_native_custody().unwrap();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let t1 = control
        .stage_eligible_audio_candidate(owner1, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    client.activate();
    client.command(PlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 64];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while client.status().unwrap().applied_revision.load(Acquire) == 0 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut block).unwrap();
            })
        }));
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(block[63], [0.25, 0.25]);

    // Live mute streamed track: predecessor's Active pages carry smooth fade-out.
    let owner2 = make_owner(&muted_project, 2);
    let t2 = control
        .stage_eligible_audio_candidate(owner2, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    let mut mute_pcm = Vec::new();
    while client.status().unwrap().applied_revision.load(Acquire) < 2 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut block).unwrap();
            })
        }));
        mute_pcm.extend_from_slice(&block);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(mute_pcm.len() >= FADE);
    assert_smooth_steps(&mute_pcm, [0.25, 0.25], 0.25 / FADE as f32 + 1e-4);
    assert_eq!(*mute_pcm.last().unwrap(), [0.0, 0.0]);
    loop {
        control.reclaim();
        if control.recover_audio_candidate(&t1).is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }

    // Live unmute streamed track: smooth ramp-in back to 0.25.
    let owner3 = make_owner(&project, 3);
    let t3 = control
        .stage_eligible_audio_candidate(owner3, vec![domain], PublicationPolicy::InheritRuntime, 0)
        .ok()
        .unwrap();
    let mut unmute_pcm = Vec::new();
    while client.status().unwrap().applied_revision.load(Acquire) < 3 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut block).unwrap();
            })
        }));
        unmute_pcm.extend_from_slice(&block);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    for _ in 0..5 {
        zero(allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|c, _| {
                c.begin_block();
                c.render(&mut block).unwrap();
            })
        }));
        unmute_pcm.extend_from_slice(&block);
    }
    assert_smooth_steps(&unmute_pcm, [0.0, 0.0], 0.25 / FADE as f32 + 1e-4);
    assert!((unmute_pcm.last().unwrap()[0] - 0.25).abs() < 1e-5);

    client.stop();
    zero(allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut block).unwrap();
        })
    }));
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    retirement.advance().unwrap();
    let control = publication.control().unwrap();
    for ticket in [&t2, &t3] {
        loop {
            control.reclaim();
            if control.recover_audio_candidate(ticket).is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(client);
    drop(publication);
    finish(&mut retirement);
}
