//! Recoverable renderer failures must close every speculative bus transaction.
use super::*;
#[test]
fn main_render_error_closes_transaction_for_immediate_handoff_and_retry() {
    use hex_lattice::conformance::owned_delay;
    use hex_project::{Command, Created, Editor};
    let mut edit = Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(source) = edit
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("source")
    };
    let Created::Track(target) = edit
        .execute(Command::AddTrack {
            name: "target".into(),
        })
        .unwrap()
    else {
        panic!("target")
    };
    let Created::Asset(asset) = edit
        .execute(Command::AddAsset {
            path: "memory/rollback.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    edit.execute(Command::PlaceClip {
        track: source,
        asset,
        start: 0,
        duration: 128,
        source_offset: 0,
        source_length: 128,
        looping: false,
    })
    .unwrap();
    edit.execute(Command::SetAudioFeed {
        source: hex_project::RoutingOwner::Track(source),
        destination: target,
        gain: 0.5,
    })
    .unwrap();
    edit.execute(Command::InsertMainDevice {
        before: None,
        state: owned_delay::state(17).unwrap(),
    })
    .unwrap();
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&owned_delay::DEFINITION];
    let registry = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let media = std::collections::BTreeMap::from([(
        asset,
        Audio::new(
            48_000,
            (0..128)
                .map(|n| [0.125 + n as f32 * 0.001, -0.25])
                .collect(),
        )
        .unwrap(),
    )]);
    let project = compile::project_with_registry(edit.project(), &media, &registry).unwrap();
    let mut failed = RendererRuntime::new(&project);
    let mut warm_position = 0;
    failed
        .render_without_pages(&project, &mut warm_position, true, &mut [[0.0; 2]; 32])
        .unwrap();
    let mut position = u64::MAX - 8;
    let original_position = position;
    let mut output = [[0.25; 2]; 4];
    // Actual compensated frame-to-beat conversion overflows after begin_block,
    // without injecting a fake error or mutating the prepared graph.
    assert!(
        failed
            .render_without_pages(&project, &mut position, true, &mut output)
            .is_err()
    );
    assert_eq!(position, original_position);
    assert_eq!(output, [[0.0; 2]; 4]);
    let mut next = RendererRuntime::new(&project);
    assert!(
        next.can_inherit_from(&project, &failed, &project),
        "failed Main transaction must be closed before any reset/retry"
    );
    next.inherit_from(&project, &failed, &project);
    let mut retry_position = warm_position;
    next.render_without_pages(&project, &mut retry_position, true, &mut output)
        .unwrap();
    let mut oracle = Renderer::new(&project);
    oracle.play();
    oracle.render(&mut [[0.0; 2]; 32]).unwrap();
    let mut expected = [[0.0; 2]; 4];
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| *sample != 0.0));
    assert_eq!(output, expected);
    assert_eq!(retry_position, oracle.position());
}
