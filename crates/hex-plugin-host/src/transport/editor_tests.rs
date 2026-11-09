//! Arm/acceptance boundaries use the original input quantum, not the read time.
use super::*;
use crate::protocol::{EditorPerform, MAX_PARAMETER_EDITS};

fn fixture() -> (Control, Callback, Worker) {
    channel(
        Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        },
        8,
    )
    .unwrap()
}

fn stage(callback: &mut Callback, position: u64, len: usize, accept: bool) {
    let origin = |index| {
        let offset = position + index as u64;
        Some(ManualParameterOrigin {
            arrangement_frame: 900 + offset % 8,
            performance_frame: 1000 + offset,
            loop_pass: offset / 8,
        })
    };
    callback.begin_physical_callback(len).unwrap();
    callback
        .stage_effect_segment(
            0,
            &[[0.5; 2]; 8][..len],
            &[],
            &[],
            &mut [[0.0; 2]; 8][..len],
            SegmentTiming {
                playing: None,
                at: |_| None,
            },
            EffectSegmentOrigins {
                authored: None,
                manual: Some(&origin),
            },
        )
        .unwrap();
    assert_eq!(
        callback.finish(accept),
        if accept { Ok(()) } else { Err(Fault::Protocol) }
    );
}

fn batch() -> EditorPerforms {
    let mut batch = EditorPerforms::default();
    for id in 0..MAX_PARAMETER_EDITS as u32 {
        batch
            .push(EditorPerform {
                id,
                gesture: 1,
                sequence: 2 + u64::from(id),
                normalized: f64::from(id) / MAX_PARAMETER_EDITS as f64,
            })
            .unwrap();
    }
    batch
}

#[test]
fn editor_scope_and_origin_are_frozen_at_the_original_quantum_start() {
    let (mut control, mut callback, mut worker) = fixture();
    let scope = NonZeroU64::MIN;
    stage(&mut callback, 0, 3, true);
    let _arm = control.arm_editor_pcm(scope).unwrap();
    assert!(matches!(
        control.arm_editor_pcm(scope),
        Err(Fault::Protocol)
    ));
    stage(&mut callback, 3, 5, true);
    let input = worker.receive_request().unwrap().unwrap();
    assert_eq!(input.editor_scope, None);
    worker.complete(input.block).unwrap();
    stage(&mut callback, 8, 8, true);
    let input = worker.receive_request().unwrap().unwrap();
    assert_eq!(input.editor_scope, Some(scope));
    let performs = batch();
    worker
        .complete_with_parameter_observations(input.block, None, Some(performs))
        .unwrap();
    stage(&mut callback, 16, 6, true);
    assert_eq!(callback.take_accepted_editor_quantum(), None);
    stage(&mut callback, 22, 4, true);
    assert_eq!(
        callback.take_accepted_editor_quantum(),
        Some(AcceptedEditorQuantum {
            binding: input.block.binding,
            sequence: 1,
            first_frame: 8,
            callback_offset: 2,
            scope,
            performs,
            restoration: None,
            lost_origin: false,
            origin: Some(ManualParameterOrigin {
                arrangement_frame: 900,
                performance_frame: 1008,
                loop_pass: 1,
            }),
        })
    );
    assert_eq!(callback.take_accepted_editor_quantum(), None);
}

#[test]
fn refused_or_ended_editor_scope_never_delivers_a_processed_reply() {
    for reject in [false, true] {
        let (mut control, mut callback, mut worker) = fixture();
        let scope = NonZeroU64::MIN;
        let arm = control.arm_editor_pcm(scope).unwrap();
        stage(&mut callback, 0, 8, true);
        let request = worker.receive_request().unwrap().unwrap();
        assert_eq!(request.editor_scope, Some(scope));
        worker
            .complete_with_parameter_observations(request.block, None, Some(batch()))
            .unwrap();
        stage(&mut callback, 8, 8, true);
        let _current = if reject {
            arm
        } else {
            drop(arm);
            assert!(matches!(
                control.arm_editor_pcm(scope),
                Err(Fault::Protocol)
            ));
            control.arm_editor_pcm(NonZeroU64::new(2).unwrap()).unwrap()
        };
        stage(&mut callback, 16, 8, !reject);
        assert_eq!(callback.take_accepted_editor_quantum(), None);
    }
}

#[test]
fn unarmed_worker_cannot_publish_editor_facts() {
    let (_control, mut callback, mut worker) = fixture();
    stage(&mut callback, 0, 8, true);
    let request = worker.receive_request().unwrap().unwrap();
    assert_eq!(
        worker.complete_with_parameter_observations(request.block, None, Some(batch())),
        Err(Fault::Protocol)
    );
    assert_eq!(callback.take_accepted_editor_quantum(), None);
}

#[test]
fn armed_empty_exchange_confirms_its_quantum_without_an_edit() {
    let (mut control, mut callback, mut worker) = fixture();
    let _arm = control.arm_editor_pcm(NonZeroU64::MIN).unwrap();
    for sequence in 0..3 {
        stage(&mut callback, sequence * 8, 8, true);
        let request = worker.receive_request().unwrap().unwrap();
        worker
            .complete_with_parameter_observations(
                request.block,
                None,
                Some(EditorPerforms::default()),
            )
            .unwrap();
    }
    let delivered = callback.take_accepted_editor_quantum().unwrap();
    assert_eq!(
        (
            delivered.sequence,
            delivered.first_frame,
            delivered.callback_offset
        ),
        (0, 0, 0)
    );
    assert!(delivered.performs.as_slice().is_empty());
}
