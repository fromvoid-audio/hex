use super::*;
use hex_devices::reverb::{REVERB_DEFINITION, parameter};

#[test]
fn reverb_power_cycle_installs_the_exact_new_room_without_callback_allocations() {
    const WARM_FRAMES: usize = 4096;
    const OFF_FRAMES: usize = 1024;
    const RESTART_FRAMES: usize = 4096;
    const RESTART_IMPULSE: usize = 512;
    const SILENCE: Frame = [0.0; 2];
    let mut source = vec![SILENCE; WARM_FRAMES + OFF_FRAMES + RESTART_FRAMES];
    source[..1024].fill([0.25, -0.125]);
    source[WARM_FRAMES + OFF_FRAMES + RESTART_IMPULSE] = [0.5, -0.5];

    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "reverb input".into(),
            path: "memory/reverb-input.wav".into(),
            source_sample_rate: RATE,
            source_frames: source.len() as u64,
            playback_frames: source.len() as u64,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("reverb reference input");
    };
    let media = BTreeMap::from([(asset, hex_engine::Audio::new(RATE, source).unwrap())]);
    let state = REVERB_DEFINITION
        .patch_parameter(
            &REVERB_DEFINITION.default_state().unwrap(),
            parameter::DRY_WET,
            ParameterValue::Float(100.0),
        )
        .unwrap();
    let state = REVERB_DEFINITION
        .patch_parameter(&state, parameter::DECAY, ParameterValue::Float(15.0))
        .unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("reverb insertion");
    };
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let wet = prepare(editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let dry = prepare(editor.project());

    let (mut control, mut callback) = channel(RATE).unwrap();
    let publish = |control: &mut Control, revision, source, policy| {
        let context = hex_engine::AudioSourceContext {
            project_generation: 6,
            editor_revision: revision,
        };
        let audio_revision = control
            .publish_unaccounted_pcm_obligation(
                Playback::try_from_prepared_with_session(source).unwrap(),
                policy,
                context.project_generation,
                true,
                false,
                Some(context),
            )
            .unwrap();
        (context, audio_revision)
    };
    let render = |callback: &mut Callback, out: &mut [Frame]| {
        for block in out.chunks_mut(BLOCK) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
    };
    publish(
        &mut control,
        1,
        Arc::clone(&wet),
        PublicationPolicy::InstallFreshRuntime,
    );
    control.command(PlaybackCommand::Play).unwrap();
    let mut warm = [SILENCE; WARM_FRAMES];
    render(&mut callback, &mut warm);
    assert!(
        warm[WARM_FRAMES - BLOCK..]
            .iter()
            .any(|&frame| frame != SILENCE)
    );

    let mut off = [SILENCE; OFF_FRAMES];
    let mut restarted = [SILENCE; RESTART_FRAMES];
    for (revision, prepared, output) in [
        (2, dry, off.as_mut_slice()),
        (3, wet, restarted.as_mut_slice()),
    ] {
        let expected = publish(
            &mut control,
            revision,
            prepared,
            PublicationPolicy::InheritRuntime,
        );
        let start = control.status.position.load(Acquire);
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(|| render(&mut callback, output));
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        render(&mut callback, output);
        let source = control.status.recording_source().unwrap();
        assert_eq!((source.context, source.audio_revision), expected);
        assert_eq!(
            control.status.position.load(Acquire),
            start + output.len() as u64
        );
        assert_eq!(control.reclaim(), 1);
    }
    assert!(off[..239].iter().any(|&frame| frame != SILENCE));
    assert_eq!(off[240..], [SILENCE; OFF_FRAMES - 240]);
    assert_eq!(restarted[..RESTART_IMPULSE], [SILENCE; RESTART_IMPULSE]);
    assert!(
        restarted[RESTART_IMPULSE..]
            .iter()
            .any(|&frame| frame != SILENCE)
    );
}
