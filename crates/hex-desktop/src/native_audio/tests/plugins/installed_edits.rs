//! Opt-in installed-instrument edit/readback integration; no app or audio device.
use super::*;
use hex_session::device_gesture::{GestureClient, PluginParameterOrigin, PluginParameterValue};
use hex_session::service::{Event, PluginParameterQuery, PluginParameterResult};

pub(super) fn query_once(
    h: &mut ResidentHarness,
    device: hex_project::DeviceId,
    page: Option<u16>,
    serial: &mut u64,
) -> PluginParameterResult {
    *serial += 1;
    let query = PluginParameterQuery {
        context: h.submitted.unwrap().1.context,
        device,
        output_identity: h.audio.as_ref().unwrap().control.output_identity().unwrap(),
        request_id: *serial,
        page_first: page,
    };
    let submitted = h.run(Request::QueryPluginParameters(query));
    if let Some(result) = submitted.plugin_parameters {
        assert_eq!(result.query, query);
        return result;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // The production device keeps running during a live controller query.
        // The headless driver must likewise advance the existing callback.
        h.pump();
        h.render_worker_step();
        std::thread::sleep(Duration::from_millis(1));
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if let Some(result) = event.plugin_parameters {
                assert_eq!(result.query, query);
                return result;
            }
        }
        assert!(
            Instant::now() < deadline,
            "installed parameter query timed out: {query:?}"
        );
        std::thread::yield_now();
    }
}

pub(super) fn query(
    h: &mut ResidentHarness,
    device: hex_project::DeviceId,
    page: Option<u16>,
    serial: &mut u64,
) -> PluginParameterResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = query_once(h, device, page, serial);
        let busy = matches!(
            result.rows.as_ref().err().map(String::as_str),
            Some(
                "vst3 controller is busy · try again" | "vst3 parameter refresh yielded to control"
            )
        );
        if !busy || Instant::now() >= deadline {
            return result;
        }
        // Commit ACK and release of the original SDK hold are distinct facts.
        // Keep the ordinary callback running until a fresh read is admitted.
        h.pump();
        h.render_worker_step();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn send(h: &mut ResidentHarness, client: &mut GestureClient) -> Event {
    let request = client.next_request().unwrap();
    client.mark_submitted();
    h.service.submit(Request::DeviceGesture(request)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        h.render_worker_step();
        if let Ok(event) = h.service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            if let Some(ack) = &event.gesture_ack {
                assert!(ack.accepted && !ack.source_lost, "{ack:?}");
                assert!(client.acknowledge(ack));
                return event;
            }
        }
        assert!(
            Instant::now() < deadline,
            "installed parameter gesture timed out"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "requires an installed Diva catalog and source-bound native helper"]
fn fourth_installed_instrument_lane_accepts_repeated_arrangement_edits() {
    use super::automation::edit_with_running_callback;
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, BeatTime, DeviceAutomationTarget,
    };
    let _native = native_fixture();
    let catalog = PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
    let selected = installed_selection(
        &catalog,
        "Diva.vst3",
        hex_plugin_catalog::PluginRole::Instrument,
    );
    let class = selected.cid;
    println!(
        "Diva class={class:?} executable_sha256={}",
        selected.executable_sha256
    );
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        44_100,
        catalog,
        20_000_000,
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "keys".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    let (applied, _) = h.applied(applied);
    let mut insert = request(&added.state, PluginEffectDestination::Instrument(track));
    insert.selection = selected;
    h.service
        .submit(Request::InsertPluginEffect(insert))
        .unwrap();
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    let device = accepted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let (mut applied, metadata) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.render_worker_step();
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire)
    );
    let point = |quarter, value| AutomationPoint {
        beat: BeatTime::from_quarters(quarter).unwrap(),
        value: AutomationValue::new(value).unwrap(),
        outgoing: AutomationInterpolation::Linear,
    };
    let edit = |revision, parameter, vst3_class, points| Request::DeviceAutomation {
        project_generation: metadata.context.project_generation,
        revision,
        device,
        target: DeviceAutomationTarget::Vst3 { parameter },
        vst3_class,
        points,
    };
    let mut state = accepted.state;
    let mut previous = Arc::clone(&state.project);
    for (parameter, end) in [
        (148, Some(0.8)),
        (86, Some(0.7)),
        (91, Some(0.65)),
        (97, None),
        (97, Some(0.6)),
        (97, Some(0.5)),
    ] {
        let mut points = vec![point(0, 0.25)];
        if let Some(value) = end {
            points.push(point(2, value));
        }
        let event = edit_with_running_callback(
            &mut h,
            edit(state.revision, parameter, Some(class), points.clone()),
        );
        assert!(
            event.error.is_none(),
            "parameter {parameter} points {}: {:?}",
            points.len(),
            event.error
        );
        assert_eq!(event.state.revision, state.revision + 1);
        assert_eq!(
            event
                .state
                .project
                .device(device)
                .unwrap()
                .automation_target(DeviceAutomationTarget::Vst3 { parameter })
                .unwrap()
                .points,
            points
        );
        (applied, _) = h.applied(applied);
        previous = Arc::clone(&state.project);
        state = event.state;
    }
    let refused = edit_with_running_callback(
        &mut h,
        edit(state.revision, 97, Some([0xa3; 16]), vec![point(0, 0.1)]),
    );
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("target no longer available"))
    );
    assert_eq!(refused.state.project, state.project);
    let undone = edit_with_running_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    assert_eq!(undone.state.project, previous);
    (applied, _) = h.applied(applied);
    let redone = edit_with_running_callback(&mut h, Request::Redo);
    assert!(redone.error.is_none(), "{:?}", redone.error);
    assert_eq!(redone.state.project, state.project);
    h.applied(applied);
}

#[test]
#[ignore = "requires explicit installed instrument/catalog/parameter IDs and source-bound native helper"]
fn distinct_instrument_parameters_commit_and_read_back_while_playing() {
    let _native = native_fixture();
    let bundle = std::env::var("HEX_TEST_VST3_INSTRUMENT_BUNDLE").unwrap();
    let catalog = PathBuf::from(std::env::var_os("HEX_TEST_VST3_INSTRUMENT_CATALOG").unwrap());
    let ids = std::env::var("HEX_TEST_VST3_PARAMETER_IDS")
        .unwrap()
        .split(',')
        .map(|id| id.parse::<u32>().unwrap())
        .collect::<Vec<_>>();
    assert!(
        (6..=8).contains(&ids.len()),
        "use six to eight distinct targets"
    );
    let unique = ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), ids.len());
    let pages = std::env::var("HEX_TEST_VST3_PARAMETER_PAGES")
        .unwrap()
        .split(',')
        .map(|page| page.parse::<u16>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        pages.len(),
        ids.len(),
        "supply one non-authoritative page hint per raw ID"
    );
    let selected = installed_selection(
        &catalog,
        &bundle,
        hex_plugin_catalog::PluginRole::Instrument,
    );
    println!(
        "installed_edit bundle={bundle} cid={:?} executable_sha256={}",
        selected.cid, selected.executable_sha256
    );
    let start = Instant::now();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
        44_100,
        catalog,
        20_000_000,
    );
    assert!(h.next().error.is_none());
    let (mut applied, _) = h.applied(0);
    let added = h.run(Request::Edit(Command::AddMidiTrack {
        name: "installed edit test".into(),
    }));
    let Some(Created::Track(track)) = added.created else {
        panic!("track")
    };
    (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(PluginEffectInsertion {
            context: added
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context(),
            selection: selected,
            destination: PluginEffectDestination::Instrument(track),
            before: None,
            track_device_name: None,
            control: Arc::new(PluginEffectControl::default()),
        }))
        .unwrap();
    let accepted = terminal(&mut h);
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.plugin_effect_complete[0].accepted);
    let device = accepted
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    (applied, _) = h.applied(applied);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    for _ in 0..4 {
        h.render_worker_step();
    }
    assert!(
        h.audio
            .as_ref()
            .unwrap()
            .status()
            .unwrap()
            .playing
            .load(Acquire),
        "the live-edit integration must actually be playing"
    );
    let output = h.audio.as_ref().unwrap().control.output_identity();
    let mut serial = 0;
    // Keep one authored target reading while the other targets are edited.
    let authored = *ids.last().unwrap();
    let page = *pages.last().unwrap();
    let result = query(&mut h, device, Some(page), &mut serial);
    let row = result
        .rows
        .unwrap()
        .into_iter()
        .find(|row| row.id == authored)
        .unwrap();
    let authored = super::automation::edit_with_running_callback(
        &mut h,
        Request::DeviceAutomation {
            project_generation: result.query.context.project_generation,
            revision: result.query.context.editor_revision,
            device,
            target: hex_project::DeviceAutomationTarget::Vst3 {
                parameter: authored,
            },
            vst3_class: row.automation_class,
            points: vec![hex_project::AutomationPoint {
                beat: hex_project::BeatTime::ZERO,
                value: hex_project::AutomationValue::new(row.normalized as f32).unwrap(),
                outgoing: hex_project::AutomationInterpolation::Step,
            }],
        },
    );
    assert!(authored.error.is_none(), "{:?}", authored.error);
    (applied, _) = h.applied(applied);
    let mut source = None;
    let mut committed = Vec::new();
    let mut client = GestureClient::default();
    for (id, page) in ids.into_iter().zip(pages) {
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "installed workflow exceeded its setup/execution budget"
        );
        let result = query(&mut h, device, Some(page), &mut serial);
        let binding = result.source.unwrap();
        assert_eq!(
            *source.get_or_insert(binding),
            binding,
            "keep the original worker during playback"
        );
        let row = result
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert!(
            row.editable && row.metadata_revision != 0,
            "target {id} is not editable: {row:?}"
        );
        let origin = PluginParameterOrigin {
            context: result.query.context,
            device,
            parameter: id,
            page_first: page,
            metadata_revision: row.metadata_revision,
            initial: PluginParameterValue::new(row.normalized).unwrap(),
        };
        let mut changed = if row.normalized < 0.5 { 0.75 } else { 0.25 };
        if row.steps > 0 {
            changed = (changed * f64::from(row.steps)).round() / f64::from(row.steps);
        }
        assert!(
            (changed - row.normalized).abs() > 0.01,
            "test must change the value"
        );
        let changed = PluginParameterValue::new(changed).unwrap();
        client.begin_plugin(origin).unwrap();
        send(&mut h, &mut client);
        client.change_plugin(changed).unwrap();
        send(&mut h, &mut client);
        client.finish_plugin(changed).unwrap();
        send(&mut h, &mut client);
        assert!(client.snapshot().is_none());
        (applied, _) = h.applied(applied);
        assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), output);
        let result = query(&mut h, device, Some(page), &mut serial);
        assert_eq!(
            result.source,
            Some(binding),
            "post-commit readback: {:?}",
            result.rows
        );
        let after = result
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert!(
            (after.normalized - changed.normalized()).abs() < 1.0e-6,
            "target {id}: SDK readback {} differs from accepted {}",
            after.normalized,
            changed.normalized()
        );
        committed.push((id, page, changed.normalized()));
        println!(
            "installed_edit parameter={id} name={:?} before={} committed={} sdk={} source={binding:?}",
            row.name,
            row.normalized,
            changed.normalized(),
            after.normalized
        );
    }
    for (id, page, value) in committed {
        let result = query(&mut h, device, Some(page), &mut serial);
        let row = result
            .rows
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert!(
            (row.normalized - value).abs() < 1.0e-6,
            "a later edit reset target {id}: {} differs from {value}",
            row.normalized
        );
    }
    println!(
        "installed_edit elapsed_seconds={:.3}",
        start.elapsed().as_secs_f64()
    );
}
