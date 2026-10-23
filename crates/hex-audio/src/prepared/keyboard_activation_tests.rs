//! Exact output activation ordering through actual prepared callbacks, including foreign roots.
use super::*;
use hex_engine::computer_keyboard::ComputerKeyboardInput;
use hex_project::{Command, Created, Editor, Project};

fn graph(
    project: &Project,
    track: hex_project::TrackId,
    input: Option<&Arc<ComputerKeyboardInput>>,
) -> Playback {
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut graph = Playback::try_from_prepared_with_session(revision).unwrap();
    if let Some(input) = input {
        graph
            .prepare_computer_keyboard(project, track, Arc::clone(input))
            .unwrap();
    }
    graph
}
fn block(callback: &mut Callback) -> [Frame; 64] {
    let mut output = [[0.0; 2]; 64];
    let count = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current
        ),
        (0, 0, 0, 0)
    );
    output
}
#[test]
fn priority_stop_preserves_only_its_own_output_keyboard_epochs_without_callback_allocation_or_free()
{
    for same_output in [true, false] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let (mut control, mut callback) = channel(48_000).unwrap();
        let (foreign, _foreign_callback) = channel(48_000).unwrap();
        let input = Arc::new(
            if same_output {
                control.new_computer_keyboard_input_on_control()
            } else {
                foreign.new_computer_keyboard_input_on_control()
            }
            .unwrap(),
        );
        input.set_key(0, Some((60, 100, 1))).unwrap();
        control
            .publish(graph(editor.project(), track, Some(&input)))
            .unwrap();
        let first = block(&mut callback);
        assert!(first.iter().flatten().any(|sample| sample.abs() > 1e-6));

        control.stop();
        input.reactivate_on_control().unwrap();
        input.set_key(0, Some((60, 100, 2))).unwrap();
        let fresh = block(&mut callback);
        if same_output {
            assert_eq!(
                fresh, first,
                "new physical epoch begins in the Stop callback"
            );
            assert!(!input.is_cancelled());
            control.stop();
            assert_eq!(
                block(&mut callback),
                [[0.0; 2]; 64],
                "held pre-Stop epoch is revoked"
            );
            input.reactivate_on_control().unwrap();
            input.set_key(0, Some((60, 100, 3))).unwrap();
            control.stop();
            input.reactivate_on_control().unwrap();
            input.set_key(0, Some((60, 100, 4))).unwrap();
            control.stop();
            assert_eq!(
                block(&mut callback),
                [[0.0; 2]; 64],
                "latest Stop revokes the entire reserved prefix"
            );
            input.reactivate_on_control().unwrap();
            control.command(PlaybackCommand::Stop).unwrap();
            assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
            assert!(input.is_cancelled(), "queued explicit Stop stays terminal");
        } else {
            assert_eq!(
                fresh, [[0.0; 2]; 64],
                "foreign output clock cannot evade this output's Stop"
            );
            assert!(input.is_cancelled());
        }
        assert!(input.reactivate_on_control().is_err());
        control
            .publish(graph(editor.project(), track, None))
            .unwrap();
        assert_eq!(block(&mut callback), [[0.0; 2]; 64]);
        control.reclaim(); // final graph/input destruction remains on control
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}
