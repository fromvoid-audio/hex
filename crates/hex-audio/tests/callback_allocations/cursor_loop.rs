//! Real command consumption and looping share the allocation probe boundary.
use super::*;

#[test]
fn absolute_cursor_before_and_after_loop_is_callback_allocation_free() -> Result<(), Box<dyn Error>>
{
    let mut editor = Editor::new(Project::new(RATE)?, 8)?;
    let Created::Imported { asset, .. } = editor.execute(Command::ImportAudioTrack {
        name: "cursor".into(),
        path: "cursor.wav".into(),
        sample_rate: RATE,
        frames: 16,
    })?
    else {
        return Err("expected audio import".into());
    };
    editor.execute(Command::SetLoopRegion {
        start: 4,
        end: 8,
        enabled: true,
    })?;
    let graph = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &BTreeMap::from([(
            asset,
            Audio::new(RATE, (1..=16).map(|n| [n as f32; 2]).collect())?,
        )]),
        Arc::new(SineTable::new()),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(graph)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "cursor publication failed")?;
    control
        .command(PlaybackCommand::PlayFrom(12))
        .map_err(|_| "post-loop start failed")?;
    let mut output = [[0.0; 2]; 6];
    measured_block(&mut callback, &mut output);
    assert_eq!(
        output,
        [
            [13.0; 2], [14.0; 2], [15.0; 2], [16.0; 2], [0.0; 2], [0.0; 2]
        ]
    );
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "stop failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, [[0.0; 2]; 6]);
    control
        .command(PlaybackCommand::PlayFrom(2))
        .map_err(|_| "pre-loop start failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(
        output,
        [[3.0; 2], [4.0; 2], [5.0; 2], [6.0; 2], [7.0; 2], [8.0; 2]]
    );
    measured_block(&mut callback, &mut output);
    assert_eq!(
        output,
        [[5.0; 2], [6.0; 2], [7.0; 2], [8.0; 2], [5.0; 2], [6.0; 2]]
    );
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
