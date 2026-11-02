//! Control-worker device gesture latency, not callback or physical-device timing.
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created};
use hex_session::{
    device_gesture::{GestureClient, GesturePhase, GestureRequest, GestureTarget},
    service::{Event, Request, Service, SubmitError},
};
use std::{
    error::Error,
    hint::black_box,
    sync::mpsc::TryRecvError,
    thread,
    time::{Duration, Instant},
};

const SAMPLE_RATE: u32 = 48_000;
const HISTORY_CAPACITY: usize = 128;
const WARMUP_ITERATIONS: usize = 5;
const TIMED_ITERATIONS: usize = 100;
const EVENT_TIMEOUT: Duration = Duration::from_secs(30);

struct Fixture {
    service: Service,
    target: GestureTarget,
    initial_gain: ParameterValue,
}

fn next_event(service: &Service) -> Result<Event, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        match service.try_event() {
            Ok(event) => return Ok(event),
            Err(TryRecvError::Disconnected) => return Err("service event channel closed".into()),
            Err(TryRecvError::Empty) if started.elapsed() < EVENT_TIMEOUT => {
                // Active polling isolates worker preparation/roundtrip time
                // from millisecond timer quantization. This benchmark wait is
                // deliberately unlike the application's paced UI event loop.
                thread::yield_now();
            }
            Err(TryRecvError::Empty) => return Err("service event timed out".into()),
        }
    }
}

fn submit(service: &Service, request: Request) -> Result<(), Box<dyn Error>> {
    service.submit(request).map_err(|error| match error {
        SubmitError::Full(_) => "service request queue is full".into(),
        SubmitError::Closed(_) => "service request queue is closed".into(),
        SubmitError::Busy(..) => "service is busy".into(),
    })
}

fn accepted_event(service: &Service, request: Request) -> Result<Event, Box<dyn Error>> {
    submit(service, request)?;
    let event = next_event(service)?;
    if let Some(error) = &event.error {
        return Err(format!("fixture request failed: {error}").into());
    }
    Ok(event)
}

fn fixture(track_count: usize) -> Result<Fixture, Box<dyn Error>> {
    let service = Service::new(SAMPLE_RATE, HISTORY_CAPACITY, 0)?;
    let initial = next_event(&service)?;
    if initial.error.is_some() {
        return Err("initial service state was rejected".into());
    }

    let type_id = DeviceTypeId::new("hex.utility")?;
    let mut targets = Vec::with_capacity(track_count);
    let mut latest_state = None;
    for index in 0..track_count {
        let event = accepted_event(
            &service,
            Request::Edit(Command::AddTrack {
                name: format!("profile track {index:03}"),
            }),
        )?;
        let Some(Created::Track(track)) = event.created else {
            return Err("track creation returned no track id".into());
        };

        let event = accepted_event(
            &service,
            Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(type_id),
                before: None,
            },
        )?;
        let Some(Created::Device(device)) = event.created else {
            return Err("utility insertion returned no device id".into());
        };

        // One empty four-beat MIDI arrangement clip per track exercises the
        // same canonical project/compiler path without adding note-render work.
        let event = accepted_event(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4)?,
            }),
        )?;
        latest_state = Some(event.state);
        targets.push(GestureTarget::DeviceParameter {
            device,
            parameter: ParameterId::new(1)?,
        });
    }
    let target = *targets.first().ok_or("profile needs at least one track")?;
    let state = latest_state.ok_or("fixture has no canonical state")?;
    let GestureTarget::DeviceParameter { device, parameter } = target else {
        return Err("profile target must be a device parameter".into());
    };
    let initial_gain = hex_engine::native_device_parameter(
        state
            .project
            .device(device)
            .ok_or("utility missing")?
            .state(),
        parameter,
    )?;
    Ok(Fixture {
        service,
        target,
        initial_gain,
    })
}

fn gesture_roundtrip(
    service: &Service,
    client: &mut GestureClient,
    target: GestureTarget,
    current: ParameterValue,
    iteration: usize,
) -> Result<ParameterValue, Box<dyn Error>> {
    let GestureTarget::DeviceParameter { device, parameter } = target else {
        return Err("profile target must be a device parameter".into());
    };
    let key = client.begin(0, target, current)?;
    let begin_request = client
        .next_request()
        .ok_or("gesture client did not produce begin")?;
    if !matches!(begin_request, GestureRequest::Begin { .. }) {
        return Err("gesture client produced a non-begin request".into());
    }
    submit(service, Request::DeviceGesture(begin_request))?;
    client.mark_submitted();

    // Queue two edits while Begin is in flight. Only the latest transient is
    // submitted after its acknowledgment, exercising the one-in-flight client.
    client.change(ParameterValue::Float(2.25))?;
    client.change(ParameterValue::Float(3.25))?;
    let begin = next_event(service)?;
    let begin_ack = begin
        .gesture_ack
        .as_ref()
        .ok_or("missing begin acknowledgment")?;
    if !begin_ack.accepted
        || begin_ack.phase != GesturePhase::Begin
        || !client.acknowledge(begin_ack)
    {
        return Err("begin gesture was not acknowledged".into());
    }

    let change_request = client
        .next_request()
        .ok_or("gesture client did not produce coalesced change")?;
    let GestureRequest::Change { value, .. } = change_request else {
        return Err("gesture client did not coalesce to a change".into());
    };
    if value != ParameterValue::Float(3.25) {
        return Err("gesture coalescer did not retain the latest value".into());
    }
    submit(service, Request::DeviceGesture(change_request))?;
    client.mark_submitted();
    let changed = next_event(service)?;
    let change_ack = changed
        .gesture_ack
        .as_ref()
        .ok_or("missing change acknowledgment")?;
    if !change_ack.accepted
        || change_ack.phase != GesturePhase::Change
        || !client.acknowledge(change_ack)
    {
        return Err("change gesture was not acknowledged".into());
    }
    let preview = changed
        .device_preview
        .as_ref()
        .ok_or("change did not publish a preview")?;
    let preview_value = hex_engine::native_device_parameter(
        preview
            .state
            .project
            .device(device)
            .ok_or("preview utility missing")?
            .state(),
        parameter,
    )?;
    if preview_value != ParameterValue::Float(3.25) {
        return Err("preview did not retain coalesced value".into());
    }

    let final_gain = ParameterValue::Float(1.1 + iteration as f32 * 0.001);
    client.finish(final_gain)?;
    let commit_request = client
        .next_request()
        .ok_or("gesture client did not produce commit")?;
    let GestureRequest::Commit { value, .. } = commit_request else {
        return Err("gesture client did not produce a commit".into());
    };
    if value != final_gain || client.snapshot().is_none_or(|snapshot| snapshot.key != key) {
        return Err("gesture commit snapshot mismatch".into());
    }
    submit(service, Request::DeviceGesture(commit_request))?;
    client.mark_submitted();
    let committed = next_event(service)?;
    let commit_ack = committed
        .gesture_ack
        .as_ref()
        .ok_or("missing commit acknowledgment")?;
    if !commit_ack.accepted
        || commit_ack.phase != GesturePhase::Commit
        || !client.acknowledge(commit_ack)
    {
        return Err("commit gesture was not acknowledged".into());
    }
    let committed_value = hex_engine::native_device_parameter(
        committed
            .state
            .project
            .device(device)
            .ok_or("committed utility missing")?
            .state(),
        parameter,
    )?;
    if committed_value != final_gain {
        return Err("committed value did not match final-value oracle".into());
    }
    black_box(committed.state.revision);
    Ok(final_gain)
}

fn percentile(values: &[u128], percentile: usize) -> u128 {
    let index = (values.len() - 1) * percentile / 100;
    values[index]
}

fn run_case(track_count: usize) -> Result<(), Box<dyn Error>> {
    let fixture = fixture(track_count)?;
    let mut client = GestureClient::default();
    let mut current = fixture.initial_gain;
    for iteration in 0..WARMUP_ITERATIONS {
        current = gesture_roundtrip(
            &fixture.service,
            &mut client,
            fixture.target,
            current,
            iteration,
        )?;
    }

    let mut samples = Vec::with_capacity(TIMED_ITERATIONS);
    for iteration in WARMUP_ITERATIONS..WARMUP_ITERATIONS + TIMED_ITERATIONS {
        let started = Instant::now();
        current = gesture_roundtrip(
            &fixture.service,
            &mut client,
            fixture.target,
            current,
            iteration,
        )?;
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let max = *samples.last().ok_or("no timed samples")?;
    println!(
        "{track_count} tracks, {track_count} utility devices, {track_count} empty 4-beat midi clips, {SAMPLE_RATE} hz, {WARMUP_ITERATIONS} warmups, {TIMED_ITERATIONS} begin/coalesced-change/commit roundtrips: p50={p50}us p95={p95}us max={max}us"
    );
    black_box(current);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    run_case(1)?;
    run_case(100)?;
    Ok(())
}
