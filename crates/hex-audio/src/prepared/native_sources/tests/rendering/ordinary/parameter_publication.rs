//! The callback admits a parameter successor only for its captured source.
use super::*;

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn gesture_publication_accepts_current_source_and_preserves_queued_pause() {
    let _serial = serial();
    for pause in [false, true] {
        let mut f = fixture_with_quantum("processing", 0, 256);
        let initial = compile(&f, true);
        publish(&mut f.control, initial, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        physical_block::<256>(&mut f);
        let (source, transport_epoch) = f.control.status.native_parameter_source(true).unwrap();
        let successor = hex_engine::AudioSourceContext {
            editor_revision: source.context.editor_revision + 1,
            ..source.context
        };
        let candidate = compile(&f, true);
        let revision = publish_in_context(
            &mut f.control,
            candidate,
            PublicationPolicy::InheritUnderNativeParameter {
                source,
                transport_epoch,
                playing: true,
            },
            successor,
        );
        if pause {
            f.control.command(PlaybackCommand::Pause).unwrap();
        }
        for _ in 0..8 {
            physical_block::<256>(&mut f);
            if f.control.status.applied_revision.load(Acquire) == revision
                || f.control
                    .status
                    .rejected_revision()
                    .is_some_and(|(r, _)| r == revision)
            {
                break;
            }
        }
        if pause {
            assert!(!f.control.status.playing.load(Acquire));
            assert_eq!(
                f.control.status.applied_revision.load(Acquire),
                source.audio_revision
            );
            assert_eq!(
                f.control.status.rejected_revision(),
                Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
            );
        } else {
            assert_eq!(f.control.status.applied_revision.load(Acquire), revision);
            assert_eq!(
                f.control.status.recording_source().unwrap().context,
                successor
            );
        }
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some())
        );
    }
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn gesture_publication_rejects_stale_witness_and_priority_stop() {
    let _serial = serial();
    for case in ["output", "revision", "epoch", "successor", "stop"] {
        let mut f = fixture_with_quantum("processing", 0, 256);
        let initial = compile(&f, true);
        publish(&mut f.control, initial, PublicationPolicy::InheritRuntime);
        f.control.command(PlaybackCommand::Play).unwrap();
        physical_block::<256>(&mut f);
        let (source, epoch) = f.control.status.native_parameter_source(true).unwrap();
        let mut witness = source;
        let mut witness_epoch = epoch;
        let mut successor = hex_engine::AudioSourceContext {
            editor_revision: source.context.editor_revision + 1,
            ..source.context
        };
        match case {
            "output" => witness.output_identity += 1,
            "revision" => witness.audio_revision += 1,
            "epoch" => witness_epoch += 1,
            "successor" => successor.editor_revision += 1,
            "stop" => {}
            _ => unreachable!(),
        }
        let candidate = compile(&f, true);
        let revision = publish_in_context(
            &mut f.control,
            candidate,
            PublicationPolicy::InheritUnderNativeParameter {
                source: witness,
                transport_epoch: witness_epoch,
                playing: true,
            },
            successor,
        );
        if case == "stop" {
            f.control.stop();
        }
        for _ in 0..8 {
            physical_block::<256>(&mut f);
            if f.control
                .status
                .rejected_revision()
                .is_some_and(|(r, _)| r == revision)
            {
                break;
            }
        }
        assert_eq!(
            f.control.status.applied_revision.load(Acquire),
            source.audio_revision,
            "{case}"
        );
        assert_eq!(
            f.control.status.rejected_revision(),
            Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged)),
            "{case}"
        );
        if case == "stop" {
            assert!(!f.control.status.playing.load(Acquire));
        }
        let returned = dispose(f);
        assert!(
            matches!(returned.disposal, native::PreparationDisposal::Reaped(retired)
            if retired.status.success() && retired.terminal.is_some()),
            "{case}"
        );
    }
}
