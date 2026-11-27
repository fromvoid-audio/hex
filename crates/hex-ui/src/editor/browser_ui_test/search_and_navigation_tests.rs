use super::*;

#[gpui::test]
fn consumed_browser_search_key_is_redacted_from_action_diagnostics(cx: &mut TestAppContext) {
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let mut log = hex_diagnostics::SessionLog::start(directory.0.join("diagnostics")).unwrap();
    cx.update(|cx| {
        crate::diagnostics::install_session_log(cx, Ok(log.handle()));
        crate::diagnostics::install_action_diagnostics(cx, true);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, cx| editor.read(cx).browser.search.read(cx).focus(window));
    visual.simulate_keystrokes("q->密");
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "密");
    });
    log.shutdown().unwrap();
    let contents = std::fs::read_to_string(log.path()).unwrap();
    assert!(contents.contains("stage=key key=\\\"redacted\\\""));
    assert!(!contents.contains("密"));
}

#[gpui::test]
fn browser_windowed_audition_matches_pcm_at_output_rate(cx: &mut TestAppContext) {
    windowed_audition_matches_resident_pcm(cx, 48_000, 0.0);
}

#[gpui::test]
fn browser_windowed_audition_matches_resampled_scrub_pcm(cx: &mut TestAppContext) {
    windowed_audition_matches_resident_pcm(cx, 44_100, 0.25);
}

#[gpui::test]
fn browser_audition_stage_diagnostics_capture_windowed_and_fallback_timings_without_paths(
    cx: &mut TestAppContext,
) {
    use hex_audio::sample_preview::Phase;
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(44_100, false).unwrap();
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let wav_path = directory.0.join("secret_local_fixture_48k.wav");
    std::fs::write(&wav_path, mono_pcm_wav(8_192)).unwrap();
    let adpcm_path = directory.0.join("secret_adpcm_fixture.wav");
    std::fs::write(
        &adpcm_path,
        include_bytes!("../../../../hex-media/tests/fixtures/adpcm_ima_wav-1.wav"),
    )
    .unwrap();
    let mut log = hex_diagnostics::SessionLog::start(directory.0.join("audition-diag")).unwrap();
    cx.update(|cx| {
        crate::diagnostics::install_session_log(cx, Ok(log.handle()));
        crate::diagnostics::install_action_diagnostics(cx, true);
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 44_100,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(wav_path.clone(), 0.25, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frames = [[0.0; 2]; 256];
        backend.render(&mut frames).unwrap();
        let phase = editor.update(&mut visual, |view, _| {
            view.poll_browser_sample_playback();
            view.browser.playback_phase
        });
        if phase == Phase::Finished {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "windowed fixture did not finish"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, cx| {
        view.play_browser_sample_from(adpcm_path.clone(), 0.0, cx);
    });
    let fallback_deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frames = [[0.0; 2]; 64];
        backend.render(&mut frames).unwrap();
        let phase = editor.update(&mut visual, |view, _| {
            view.poll_browser_sample_playback();
            view.browser.playback_phase
        });
        if phase == Phase::Playing {
            break;
        }
        assert!(
            std::time::Instant::now() < fallback_deadline,
            "resident fallback fixture did not start playing"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, _| {
        view.cancel_browser_sample_playback();
        view.audio.take().unwrap().control.close();
    });
    backend.close();
    drop(output);
    log.shutdown().unwrap();
    let contents = std::fs::read_to_string(log.path()).unwrap();
    assert!(!contents.contains("secret_local_fixture_48k"));
    assert!(!contents.contains("secret_adpcm_fixture"));
    let windowed_stages: Vec<&str> = contents
        .lines()
        .filter(|line| line.contains("browser_audition v=1") && line.contains("generation=1 "))
        .filter_map(|line| {
            line.split("stage=")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
        })
        .collect();
    assert_eq!(
        windowed_stages,
        vec![
            "click",
            "worker_start",
            "header_read",
            "pcm_ready",
            "published",
            "callback_ack",
            "terminal",
        ],
        "windowed trace stages must appear in causal order:\n{contents}"
    );
    assert!(
        contents.contains("generation=1")
            && contents.contains("source_rate=48000 output_rate=44100 rate_matched=false")
    );
    let fallback_stages: Vec<&str> = contents
        .lines()
        .filter(|line| line.contains("browser_audition v=1") && line.contains("generation=2 "))
        .filter_map(|line| {
            line.split("stage=")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
        })
        .collect();
    assert_eq!(
        fallback_stages,
        vec![
            "click",
            "worker_start",
            "header_read",
            "worker_start",
            "pcm_ready",
            "published",
            "callback_ack",
            "terminal",
        ],
        "resident fallback trace must separate failed windowed header probe from resident playback:\n{contents}"
    );
    assert!(contents.contains("outcome=fallback_resident"));
}

#[gpui::test]
fn browser_windowed_audition_supersession_wakes_backpressure_wait_without_timer_delay(
    cx: &mut TestAppContext,
) {
    use hex_audio::sample_preview::Phase;
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, directory, first) = browser_drag_fixture(cx);
    // 20_000 frames > 2 * BROWSER_PREVIEW_WINDOW_FRAMES, so window 2 blocks in
    // WindowDelivery::Full while window 0 is playing and window 1 is queued.
    std::fs::write(&first, mono_pcm_wav(20_000)).unwrap();
    let second = directory.0.join("instant_wake_second.wav");
    let mut bytes = mono_pcm_wav(10_000);
    for sample in bytes[44..].chunks_exact_mut(2) {
        sample.copy_from_slice(&12_000_i16.to_le_bytes());
    }
    std::fs::write(&second, bytes).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(first, 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        if frame == [[4_000.0 / 32_768.0; 2]; 1] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "first windowed audition did not start"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    // Pump until window 1 fills the incoming slot and window 2 parks on the 5 ms backpressure timer.
    visual.run_until_parked();
    assert!(
        editor.read_with(&visual, |view, _| view.browser.playback_in_flight.is_some()),
        "first audition should be waiting on window backpressure"
    );
    // Switch samples and park WITHOUT advancing the fake clock by 5 ms.
    editor.update(&mut visual, |view, cx| {
        view.play_browser_sample_from(second, 0.0, cx);
    });
    visual.run_until_parked();
    let mut frame = [[0.0; 2]; 1];
    backend.render(&mut frame).unwrap();
    assert_eq!(
        frame,
        [[12_000.0 / 32_768.0; 2]; 1],
        "replacement sample must start immediately without waiting for the old 5 ms backpressure timer"
    );
    editor.read_with(&visual, |view, _| {
        assert_eq!(
            view.audio
                .as_ref()
                .unwrap()
                .control
                .preview_snapshot()
                .unwrap()
                .phase,
            Phase::Playing
        );
    });
    editor.update(&mut visual, |view, _| {
        view.cancel_browser_sample_playback();
        view.audio.take().unwrap().control.close();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn browser_windowed_audition_supersession_and_stop_silence_the_old_source(cx: &mut TestAppContext) {
    use hex_audio::sample_preview::Phase;
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, directory, first) = browser_drag_fixture(cx);
    std::fs::write(&first, mono_pcm_wav(20_000)).unwrap();
    let second = directory.0.join("second.wav");
    let mut bytes = mono_pcm_wav(20_000);
    for sample in bytes[44..].chunks_exact_mut(2) {
        sample.copy_from_slice(&8_000_i16.to_le_bytes());
    }
    std::fs::write(&second, bytes).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(first, 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        if frame == [[4_000.0 / 32_768.0; 2]; 1] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "first source did not start"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, cx| {
        view.play_browser_sample_from(second, 0.0, cx)
    });
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        assert!(
            frame == [[0.0; 2]; 1] || frame == [[8_000.0 / 32_768.0; 2]; 1],
            "old source returned after supersession"
        );
        let playing = editor.read_with(&visual, |view, _| {
            view.audio
                .as_ref()
                .unwrap()
                .control
                .preview_snapshot()
                .unwrap()
                .phase
                == Phase::Playing
        });
        if playing {
            assert_eq!(frame, [[8_000.0 / 32_768.0; 2]; 1]);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replacement source did not start"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, _| view.cancel_browser_sample_playback());
    let mut frame = [[1.0; 2]; 1];
    backend.render(&mut frame).unwrap();
    assert_eq!(frame, [[0.0; 2]; 1]);
    editor.update(&mut visual, |view, _| {
        view.audio.take().unwrap().control.close()
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn browser_windowed_audition_reports_source_change_and_stops(cx: &mut TestAppContext) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    std::fs::write(&path, mono_pcm_wav(20_000)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(path.clone(), 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        if frame == [[4_000.0 / 32_768.0; 2]; 1] {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "source did not start");
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    std::fs::write(&path, mono_pcm_wav(20_001)).unwrap();
    loop {
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
        visual.run_until_parked();
        let mut frames = [[0.0; 2]; 128];
        backend.render(&mut frames).unwrap();
        let error = editor.read_with(&visual, |view, _| {
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("audio file changed"))
        });
        if error {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "changed source was not reported"
        );
    }
    let mut frame = [[1.0; 2]; 1];
    backend.render(&mut frame).unwrap();
    assert_eq!(frame, [[0.0; 2]; 1]);
    editor.update(&mut visual, |view, _| {
        view.audio.take().unwrap().control.close()
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn browser_adpcm_audition_keeps_resident_fallback(cx: &mut TestAppContext) {
    use hex_audio::sample_preview::Phase;
    use std::sync::atomic::AtomicBool;
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    std::fs::write(
        &path,
        include_bytes!("../../../../hex-media/tests/fixtures/adpcm_ima_wav-1.wav"),
    )
    .unwrap();
    assert!(
        hex_session::service::PreviewWindowSource::open(&path, 48_000, &AtomicBool::new(false))
            .is_err()
    );
    let expected = hex_session::service::PreviewSources::default()
        .prepare_playback(&path, 48_000, &AtomicBool::new(false))
        .unwrap();
    assert!(!expected.frames().is_empty());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(path, 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        let playing = editor.read_with(&visual, |view, _| {
            view.audio
                .as_ref()
                .unwrap()
                .control
                .preview_snapshot()
                .unwrap()
                .phase
                == Phase::Playing
        });
        if playing {
            assert_eq!(frame[0], expected.frames()[0]);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "resident fallback did not play"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, _| {
        view.cancel_browser_sample_playback();
        view.audio.take().unwrap().control.close();
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn stale_browser_audition_cannot_cancel_a_replacement_output_or_request(cx: &mut TestAppContext) {
    use hex_audio::sample_preview::Phase;
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    std::fs::write(&path, mono_pcm_wav(20_000)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let mut client = output.take_client().unwrap();
    let identity = client.output_identity().unwrap();
    let first = client.preview_request().unwrap();
    client
        .preview_publish(
            first,
            hex_engine::Audio::new(48_000, vec![[0.5; 2]; 16]).unwrap(),
        )
        .unwrap();
    editor.update(&mut visual, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.browser.playback_output = Some((identity.wrapping_add(1), first));
        view.browser.playback_phase = Phase::Playing;
        assert!(view.poll_browser_sample_playback());
    });
    let mut frame = [[0.0; 2]; 1];
    backend.render(&mut frame).unwrap();
    assert_eq!(
        frame, [[0.5; 2]; 1],
        "old output identity canceled the new output"
    );
    editor.update(&mut visual, |view, _| {
        let audio = view.audio.as_mut().unwrap();
        let second = audio.control.preview_request().unwrap();
        audio
            .control
            .preview_publish(
                second,
                hex_engine::Audio::new(48_000, vec![[0.75; 2]; 16]).unwrap(),
            )
            .unwrap();
        view.browser.playback_output = Some((identity, first));
        view.browser.playback_phase = Phase::Playing;
        assert!(view.poll_browser_sample_playback());
    });
    backend.render(&mut frame).unwrap();
    assert_eq!(
        frame, [[0.75; 2]; 1],
        "old generation canceled a newer preview"
    );
    editor.update(&mut visual, |view, cx| {
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(path, 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        backend.render(&mut frame).unwrap();
        if frame == [[4_000.0 / 32_768.0; 2]; 1] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "windowed source did not start"
        );
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    editor.update(&mut visual, |view, _| {
        let control = &mut view.audio.as_mut().unwrap().control;
        let token = control.preview_request().unwrap();
        control
            .preview_publish(
                token,
                hex_engine::Audio::new(48_000, vec![[0.875; 2]; 64]).unwrap(),
            )
            .unwrap();
    });
    let mut replacement_started = false;
    for _ in 0..8 {
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
        visual.run_until_parked();
        backend.render(&mut frame).unwrap();
        if frame == [[0.875; 2]; 1] {
            replacement_started = true;
        } else {
            assert!(
                !replacement_started && frame == [[0.0; 2]; 1],
                "old window producer canceled the newer request"
            );
        }
    }
    assert!(replacement_started, "replacement preview did not start");
    editor.update(&mut visual, |view, _| {
        view.audio.take().unwrap().control.close()
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn windowed_browser_audition_handles_device_fault_and_output_replacement(cx: &mut TestAppContext) {
    let _serial = hex_audio::worker_fixture::serial_output();
    let (mut output, mut backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let (handle, editor, _, _directory, path) = browser_drag_fixture(cx);
    std::fs::write(&path, mono_pcm_wav(20_000)).unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |view, cx| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "test output".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.audio_pub.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.browser.section = BrowserSection::Samples;
        view.play_browser_sample_from(path.clone(), 0.0, cx);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        backend.render(&mut frame).unwrap();
        if frame == [[4_000.0 / 32_768.0; 2]; 1] {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "source did not start");
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
    }
    backend.report_device_error().unwrap();
    editor.update(&mut visual, |view, _| {
        assert!(view.poll_browser_sample_playback());
        assert_eq!(
            view.browser.playback_phase,
            hex_audio::sample_preview::Phase::Unavailable
        );
        view.audio.take().unwrap().control.close();
    });
    backend.close();
    drop(output);
    let (mut replacement, mut replacement_backend) =
        hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
    let mut client = replacement.take_client().unwrap();
    let token = client.preview_request().unwrap();
    client
        .preview_publish(
            token,
            hex_engine::Audio::new(48_000, vec![[0.625; 2]; 64]).unwrap(),
        )
        .unwrap();
    editor.update(&mut visual, |view, _| {
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "replacement".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
    });
    for _ in 0..8 {
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(5));
        visual.run_until_parked();
        let mut frame = [[0.0; 2]; 1];
        replacement_backend.render(&mut frame).unwrap();
        assert_eq!(
            frame, [[0.625; 2]; 1],
            "old browser task affected replacement output"
        );
    }
    editor.update(&mut visual, |view, _| {
        view.audio.take().unwrap().control.close()
    });
    replacement_backend.close();
    drop(replacement);
}

#[gpui::test]
fn command_f_opens_all_search_and_replaces_query_from_editor_and_results(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, track, _directory, _) = browser_drag_fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.browser.open = false;
            view.browser.section = BrowserSection::Projects;
            view.browser
                .listing
                .as_mut()
                .expect("listing")
                .entries
                .push(LibraryEntry {
                    name: "Loop.hex".into(),
                    path: view.browser.directory.join("Loop.hex"),
                    kind: EntryKind::Project,
                });
            view.focus.focus(window);
            cx.notify();
        });
    })
    .expect("focus editor");

    cx.simulate_keystrokes(handle, "cmd-f");
    cx.update_window(handle, |_, window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(view.browser.open);
            assert_eq!(view.browser.section, BrowserSection::All);
            assert!(
                view.browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
            assert!(!view.browser.results_focused);
        });
    })
    .expect("search focus");
    cx.simulate_input(handle, "LOOP");
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.browser_visible_entries(cx).len(),
            2,
            "all includes samples and projects"
        );
        assert!(view.browser_visible_devices(cx).is_empty());
    });

    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.browser.selected_index, 0,
            "first down selects first match"
        )
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| assert_eq!(view.browser.selected_index, 1));
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "mix");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "mix");
        assert!(view.browser_visible_entries(cx).is_empty());
        let devices = view.browser_visible_devices(cx);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].1.name.to_lowercase(), "mix");
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "mix");
        assert!(!view.browser.results_focused);
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, _| assert_eq!(view.browser.selected_index, 0));

    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.mode = super::super::super::EditorMode::Session;
            view.open_track_context_menu(point(px(100.0), px(100.0)), Some(track), window, cx);
        });
    })
    .expect("open context menu");
    cx.simulate_keystrokes(handle, "cmd-f");
    editor.read_with(cx, |view, _| assert!(view.context_menu.is_none()));
    cx.simulate_input(handle, "écho");
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "loop");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "loop")
    });
    cx.simulate_keystrokes(handle, "escape");
    editor.read_with(cx, |view, cx| {
        assert!(view.browser.search.read(cx).text().is_empty())
    });
}

#[gpui::test]
fn command_f_commits_clip_name_draft_then_routes_typing_to_browser(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (project, _, _, clip) = crate::editor::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.selected_clip = Some(clip);
            view.begin_clip_name(window, cx);
        });
    })
    .expect("rename focus");
    cx.simulate_input(handle, "verse");
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "mix");
    editor.read_with(cx, |view, cx| {
        assert!(view.clip_name_edit.is_none());
        assert!(matches!(&view.test_probes.last_enqueued_edit,
            Some(Command::RenameClip { clip: target, name }) if *target == clip && name == "verse"));
        assert_eq!(view.browser.search.read(cx).text(), "mix");
        assert_eq!(view.browser.section, BrowserSection::All);
    });
}

#[gpui::test]
fn command_f_replaces_entire_query_during_ime_composition(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler;
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "loop");
    cx.update_window(handle, |_, window, cx| {
        let search = editor.read(cx).browser.search.clone();
        search.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "音", Some(1..1), window, cx);
            assert!(input.marked_text_range(window, cx).is_some());
        });
    })
    .unwrap();
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "mix");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "mix");
    });
}

#[gpui::test]
fn command_f_caret_movement_preserves_result_scroll(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("sample");
    for index in 0..50 {
        std::fs::write(
            directory.0.join(format!("sample-{index}.wav")),
            mono_pcm_wav(16),
        )
        .unwrap();
    }
    editor.update(visual, |view, cx| view.refresh_sample_catalog(cx));
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser
            .scroll
            .scroll_to_item_strict(20, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    let first_device = "browser-device-instrument-hex.instrument.sampler";
    assert!(visual.debug_bounds(first_device).is_none());
    assert!(visual.debug_bounds("browser-entry-20").is_some());
    assert!(visual.debug_bounds("browser-entry-0").is_none());
    visual.simulate_keystrokes("left shift-left");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-20").is_some(),
        "caret movement must preserve viewport"
    );
    assert!(visual.debug_bounds("browser-entry-0").is_none());
    visual.simulate_keystrokes("end backspace");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(first_device).is_some(),
        "changed query reveals its first device result"
    );
    editor.read_with(visual, |view, cx| {
        assert_eq!(view.browser.search.read(cx).text(), "sampl");
        assert!(!view.browser.results_focused);
    });
}

#[gpui::test]
fn command_f_device_highlight_waits_for_result_focus(cx: &mut TestAppContext) {
    use crate::theme;
    cx.update(super::super::super::bind_editor_keys);
    let (handle, _, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("mix");
    visual.run_until_parked();
    let selector = "browser-device-effect-hex.utility";
    assert_eq!(
        visual.rendered_background(selector),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Surface)).into())
    );
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_background(selector),
        Some(gpui::rgb(theme::color(theme::PaletteRole::Secondary)).into())
    );
}

#[gpui::test]
fn command_f_category_click_keeps_query_but_moves_real_focus_in_session(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    for category in ["samples", "devices", "projects", "all"] {
        visual.simulate_keystrokes("cmd-f");
        visual.simulate_input("mi");
        visual.run_until_parked();
        let selector = Box::leak(format!("browser-section-{category}").into_boxed_str());
        let bounds = visual.debug_bounds(selector).expect("category");
        visual.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            editor.read_with(cx, |view, cx| {
                assert_eq!(
                    view.browser_key_context(window, cx),
                    "hex_browser",
                    "category {category}"
                )
            });
        });
        visual.simulate_input("x");
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "mi")
        });
        let field = visual.debug_bounds("browser-search").unwrap();
        visual.simulate_click(field.center(), Modifiers::default());
        visual.simulate_keystrokes("right");
        visual.simulate_input("x");
        editor.read_with(visual, |view, cx| {
            assert_eq!(view.browser.search.read(cx).text(), "mix")
        });
    }
    visual.simulate_keystrokes("down enter");
    editor.read_with(visual, |view, _| {
        assert!(view.status.contains("drag a browser device"));
        assert!(view.test_probes.last_transport_command.is_none());
    });
}

#[gpui::test]
fn command_f_escape_keeps_navigation_until_explicit_text_entry(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f down right");
    visual.run_until_parked();
    visual.simulate_keystrokes("down");
    let generation = editor.read_with(visual, |view, _| {
        assert!(view.browser.results_focused);
        assert_eq!(
            view.browser.playback_generation, 1,
            "Down auto-auditions the sample"
        );
        view.browser.playback_generation
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert!(!view.browser.results_focused);
            assert_eq!(view.browser_key_context(window, cx), "hex_browser");
            assert!(
                !view
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
            );
        });
    });
    // Unbound printable keys leave result navigation untouched. `r` now has
    // its explicit random-sample action in this context.
    visual.simulate_input("text");
    editor.read_with(visual, |view, cx| {
        assert!(view.browser.search.read(cx).text().is_empty());
        assert_eq!(view.browser.playback_generation, generation);
    });
    let field = visual.debug_bounds("browser-search").unwrap();
    visual.simulate_click(field.center(), Modifiers::default());
    visual.simulate_keystrokes("right");
    visual.simulate_input("loop");
    editor.read_with(visual, |view, cx| {
        assert_eq!(
            view.browser.playback_generation, generation,
            "Right in the search field must only move the caret"
        );
        assert_eq!(view.browser.search.read(cx).text(), "loop");
    });
}

#[gpui::test]
fn command_f_scan_completion_preserves_selected_device(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    cx.simulate_keystrokes(handle, "cmd-f");
    cx.simulate_input(handle, "mix");
    let request = editor.update(cx, |view, cx| {
        view.browser.listing = None;
        view.browser.loading = true;
        view.browser.scans = Default::default();
        let request = view
            .browser
            .scans
            .request(view.browser.directory.clone())
            .unwrap();
        cx.notify();
        request
    });
    cx.simulate_keystrokes(handle, "down");
    editor.read_with(cx, |view, cx| {
        assert_eq!(view.browser_selected_label(cx).to_lowercase(), "mix")
    });
    editor.update(cx, |view, cx| {
        let directory = view.browser.directory.clone();
        view.browser_scan_finished(
            request,
            Ok(DirectoryListing {
                directory: directory.clone(),
                entries: vec![LibraryEntry {
                    name: "mix.hex".into(),
                    path: directory.join("mix.hex"),
                    kind: EntryKind::Project,
                }],
                enumerated_entries: 1,
                truncated: false,
            }),
            cx,
        );
        cx.notify();
    });
    editor.read_with(cx, |view, cx| {
        assert_eq!(
            view.browser_selected_label(cx).to_lowercase(),
            "mix",
            "late files must not retarget device selection"
        );
        assert_eq!(view.browser.selected_index, 0);
    });
    cx.simulate_keystrokes(handle, "enter");
    editor.read_with(cx, |view, _| {
        assert!(view.status.contains("drag a browser device"))
    });
}

#[gpui::test]
fn command_f_device_click_retains_browser_keys_in_session(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, _directory, _) = browser_drag_fixture(cx);
    editor.update(cx, |view, cx| {
        view.mode = crate::editor::EditorMode::Session;
        cx.notify();
    });
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    visual.simulate_input("mix");
    visual.run_until_parked();
    let row = visual
        .debug_bounds("browser-device-effect-hex.utility")
        .expect("utility result");
    visual.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| {
            assert_eq!(view.browser_key_context(window, cx), "hex_browser")
        });
    });
    visual.simulate_keystrokes("enter escape");
    editor.read_with(visual, |view, cx| {
        assert!(
            view.test_probes.last_transport_command.is_none(),
            "browser enter must not launch session playback"
        );
        assert!(view.status.contains("drag a browser device"));
        assert!(view.browser.search.read(cx).text().is_empty());
    });
}

#[gpui::test]
fn command_f_category_switch_scrolls_first_result_into_view(cx: &mut TestAppContext) {
    cx.update(super::super::super::bind_editor_keys);
    let (handle, editor, _, directory, _) = browser_drag_fixture(cx);
    let visual = &mut VisualTestContext::from_window(handle, cx);
    visual.simulate_keystrokes("cmd-f");
    for index in 0..50 {
        std::fs::write(
            directory.0.join(format!("sample-{index}.wav")),
            mono_pcm_wav(16),
        )
        .unwrap();
        std::fs::write(directory.0.join(format!("project-{index}.hex")), []).unwrap();
    }
    editor.update(visual, |view, cx| {
        view.refresh_sample_catalog(cx);
        view.request_browser_scan(directory.0.clone(), cx);
    });
    visual.run_until_parked();
    let samples = visual.debug_bounds("browser-section-samples").unwrap();
    visual.simulate_mouse_down(samples.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(samples.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let root = visual.debug_bounds("browser-disclosure-0").unwrap();
    visual.simulate_mouse_down(root.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(root.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(visual, |view, cx| {
        view.browser
            .scroll
            .scroll_to_item_strict(20, gpui::ScrollStrategy::Top);
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-20").is_some(),
        "scroll target is painted"
    );
    assert!(
        visual.debug_bounds("browser-entry-0").is_none(),
        "first row is outside the painted viewport"
    );
    let projects = visual.debug_bounds("browser-section-projects").unwrap();
    visual.simulate_mouse_down(projects.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(projects.center(), MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("browser-entry-0").is_some(),
        "new category's first result must be visible"
    );
}
