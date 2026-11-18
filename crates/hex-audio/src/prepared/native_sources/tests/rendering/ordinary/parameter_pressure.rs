//! Real native source with an ordinary streamed successor under page pressure.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn native_parameter_successor_under_stream_pressure_preserves_playback() {
    pressure_case(false);
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn queued_play_survives_stale_native_parameter_successor() {
    pressure_case(true);
}

fn pressure_case(queued_play: bool) {
    let _serial = serial();
    let mut f = fixture_with_quantum("processing", 2, 256);
    let mut ids = Vec::new();
    for name in ["current", "candidate"] {
        let Created::Track(track) = f
            .editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Asset(asset) = f
            .editor
            .execute(Command::AddAsset {
                path: format!("{name}.wav").into(),
                sample_rate: 48_000,
                frames: 12288,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        f.editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: 12288,
                source_offset: 0,
                source_length: 12288,
                looping: false,
            })
            .unwrap();
        ids.push((track, asset));
    }
    f.editor
        .execute(Command::Mute {
            track: ids[1].0,
            muted: true,
        })
        .unwrap();
    let current = f.editor.project().clone();
    f.editor
        .execute(Command::Mute {
            track: ids[0].0,
            muted: true,
        })
        .unwrap();
    f.editor
        .execute(Command::Mute {
            track: ids[1].0,
            muted: false,
        })
        .unwrap();
    let successor = f.editor.project().clone();
    let wav_a =
        crate::prepared::eligible_publication::tests::stream_fixture::Wav::with_frames(12288);
    let wav_b =
        crate::prepared::eligible_publication::tests::stream_fixture::Wav::with_frames(12288);
    let a = wav_a.definition(ids[0].1, 11);
    let b = wav_b.definition(ids[1].1, 11);
    let mut sources: BTreeMap<_, _> = f
        .media
        .iter()
        .map(|(id, audio)| {
            (
                *id,
                hex_engine::compile::PreparedAudioSource::Resident(audio.clone()),
            )
        })
        .collect();
    sources.insert(
        ids[0].1,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(a.metadata.clone())),
    );
    sources.insert(
        ids[1].1,
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(b.metadata.clone())),
    );
    let mut runtime =
        hex_stream::StreamRuntime::new(hex_stream::StreamConfig::with_pool_pages(3), vec![a, b])
            .unwrap();
    runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
    let domain = f
        .control
        .register_audio_candidate_domain_with_readiness(
            runtime.split().ok().unwrap(),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|refusal| panic!("domain: {}", refusal.reason));
    f.data
        .owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let compile_streamed = |f: &OrdinaryFixture, project: &Project| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let result = crate::plugin_effects::Hosting::prepare_on_control(
                &f.control,
                std::slice::from_ref(&f.binding),
                &[],
                &mut |effects| {
                    effects.prepare_registered_source_revision(
                        project,
                        &sources,
                        11,
                        Arc::new(hex_engine::SineTable::new()),
                        None,
                        &hex_engine::audio_transform::AudioTransformPreparation::default(),
                        &f.resources,
                        100_000,
                        &AtomicBool::new(false),
                        None,
                    )
                },
            );
            match result {
                Ok(revision) => {
                    return Playback::try_from_prepared_with_device_resources(
                        Arc::new(revision),
                        &f.resources,
                    )
                    .unwrap();
                }
                Err(reason) if reason.contains("busy") => {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                Err(reason) => panic!("{reason}"),
            }
        }
    };
    let original = compile_streamed(&f, &current);
    let revision = publish(&mut f.control, original, PublicationPolicy::InheritRuntime);
    f.control.command(PlaybackCommand::Play).unwrap();
    let root = f.control.native_candidate_root();
    let render = |f: &mut OrdinaryFixture| {
        let pcm = physical_block::<256>(f);
        let mut committed = None;
        f.data.owner_mut().unwrap().run(|callback, _| {
            committed = callback.native_sources.test_committed_frames(f.binding);
        });
        replacement::wait_for_blocks(&root, f.binding, committed.unwrap() / 256);
        pcm
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut baseline = [[0.0; 2]; 256];
    while f.control.status.position.load(Acquire) < 4096 {
        baseline = render(&mut f);
        assert!(Instant::now() < deadline, "current stream did not warm");
        std::thread::yield_now();
    }
    if queued_play {
        f.control.command(PlaybackCommand::Pause).unwrap();
        while f.control.status.playing.load(Acquire) {
            render(&mut f);
            assert!(Instant::now() < deadline, "Pause did not finish");
            std::thread::yield_now();
        }
    }
    let (source, transport_epoch) = f
        .control
        .status
        .native_parameter_source(!queued_play)
        .unwrap();
    assert_eq!(source.audio_revision, revision);
    let next = compile_streamed(&f, &successor);
    let next_revision = publish_in_context(
        &mut f.control,
        next,
        PublicationPolicy::InheritUnderNativeParameter {
            source,
            transport_epoch,
            playing: !queued_play,
        },
        hex_engine::AudioSourceContext {
            editor_revision: source.context.editor_revision + 1,
            ..source.context
        },
    );
    if queued_play {
        let ticket = f
            .control
            .command_with_ticket(PlaybackCommand::Play)
            .unwrap();
        while f
            .control
            .status
            .rejected_revision()
            .is_none_or(|(id, _)| id != next_revision)
            || !f.control.status.playing.load(Acquire)
        {
            render(&mut f);
            assert!(
                Instant::now() < deadline,
                "queued Play or stale candidate did not settle"
            );
            std::thread::yield_now();
        }
        assert!(f.control.status.consumed_command.load(Acquire) >= ticket);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            f.control.status.rejected_revision(),
            Some((next_revision, PlaybackHandoffRejection::RuntimeFenceChanged))
        );
    } else {
        let before = f.control.status.position.load(Acquire);
        let pcm = render(&mut f);
        assert!(
            baseline
                .iter()
                .all(|frame| frame[0] != 0.0 && frame[1] != 0.0)
        );
        // This SDK fixture increases its gain by 1/8 per completed block;
        // the constant 1/4 input therefore rises by exactly 1/32 per frame.
        for (actual, previous) in pcm.iter().zip(baseline) {
            assert_eq!(*actual, [previous[0] + 0.03125, previous[1] + 0.03125]);
        }
        assert_eq!(f.control.status.position.load(Acquire), before + 256);
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        assert_eq!(f.control.status.rejected_revision(), None);
        assert!(f.control.status.publication_pending.load(Acquire));
        assert!(!f.control.status.stream_buffering.load(Acquire));
        f.control.stop();
        while f
            .control
            .status
            .rejected_revision()
            .is_none_or(|(id, _)| id != next_revision)
        {
            render(&mut f);
            assert!(
                Instant::now() < deadline,
                "priority Stop did not retire successor"
            );
            std::thread::yield_now();
        }
        assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            f.control.status.rejected_revision(),
            Some((next_revision, PlaybackHandoffRejection::RuntimeFenceChanged))
        );
        assert!(!f.control.status.playing.load(Acquire));
    }
    assert!(f.control.retire_audio_candidate_domain(domain));
    f.control.request_shutdown();
    while f.control.recover_audio_candidate_domain(domain).is_none() {
        physical_block::<256>(&mut f);
        f.control.reclaim();
        assert!(Instant::now() < deadline, "domain retirement");
        std::thread::yield_now();
    }
    let returned = dispose(f);
    assert!(
        matches!(returned.disposal, native::PreparationDisposal::Reaped(retired) if retired.status.success())
    );
}
