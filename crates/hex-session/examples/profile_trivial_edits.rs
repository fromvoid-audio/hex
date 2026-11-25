//! Control-worker latency benchmark for #13245 trivial edits on a realistic
//! 32-track project containing Sampler instruments, resident audio clips,
//! MIDI clips with built-in effects, and return sends.
//!
//! Measures control-worker roundtrip CPU/wall time (not audio callback or
//! physical-device deadlines) comparing the incremental fast paths (Mute/Solo,
//! Track Gain drag ticks, MIDI note moves) against the full-compile worker path
//! on the exact same 32-track resident project.
use hex_engine::{
    PreparedPlaybackObservation,
    native_device_resources::{DeviceResourceAdmission, PreparedDeviceResources},
    playback::Playback,
};
use hex_lattice_contract::{DeviceTypeId, ParameterValue};
use hex_project::{BeatTime, Command, Created, InstrumentLoadPlacement};
use hex_session::{
    ResidentNativeReservation,
    device_gesture::{GestureClient, GesturePhase, GestureTarget},
    service::{
        CanonicalAudioSourceSnapshot, Event, NativeSamplerReservationHold, Request,
        ResidentPublicationAcceptance, ResidentPublicationMetadata, ResidentPublicationPolicy,
        ResidentPublicationReadiness, ResidentPublicationRefusal, ResidentRuntimeLoop,
        ResidentSourceOrigin, SamplerLoadSlot, SamplerLoadTarget, Service, SubmitError,
        WorkerAudioSink,
    },
};
use std::{
    error::Error,
    hint::black_box,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::TryRecvError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SAMPLE_RATE: u32 = 48_000;
const HISTORY_CAPACITY: usize = 64;
const WARMUP_ITERATIONS: usize = 10;
const TIMED_ITERATIONS: usize = 60;
const EVENT_TIMEOUT: Duration = Duration::from_secs(30);

struct BenchmarkSink {
    active: Option<(
        Playback,
        PreparedDeviceResources,
        Arc<PreparedPlaybackObservation>,
    )>,
    readiness: ResidentPublicationReadiness,
    next_audio_revision: u64,
}

impl BenchmarkSink {
    fn new() -> Self {
        Self {
            active: None,
            readiness: ResidentPublicationReadiness::NoOutput,
            next_audio_revision: 0,
        }
    }
}

impl WorkerAudioSink for BenchmarkSink {
    fn resident_publication_enabled(&self) -> bool {
        true
    }

    fn resident_publication_readiness(&mut self) -> ResidentPublicationReadiness {
        self.readiness
    }

    fn resident_admission(&self) -> [DeviceResourceAdmission<'_>; 3] {
        [
            self.active.as_ref().map_or(
                DeviceResourceAdmission::default(),
                |(playback, resources, _)| {
                    DeviceResourceAdmission::unpublished_on_control(playback, Some(resources))
                },
            ),
            DeviceResourceAdmission::default(),
            DeviceResourceAdmission::default(),
        ]
    }

    fn native_sampler_hold(&self) -> NativeSamplerReservationHold {
        let bytes = self.active.as_ref().map_or(0, |(_, _, view)| {
            view.retained_bytes_on_control().unwrap_or(0)
        });
        NativeSamplerReservationHold {
            retained_bytes: bytes,
            construction_bytes: bytes,
            ..NativeSamplerReservationHold::default()
        }
    }

    #[allow(clippy::result_large_err)]
    fn publish_resident(
        &mut self,
        playback: Playback,
        resources: Option<&PreparedDeviceResources>,
        observation: Arc<PreparedPlaybackObservation>,
        metadata: ResidentPublicationMetadata,
        _policy: ResidentPublicationPolicy,
        metadata_limit: usize,
        admit: &mut dyn FnMut(&[DeviceResourceAdmission<'_>]) -> Result<(), String>,
    ) -> Result<ResidentPublicationAcceptance, ResidentPublicationRefusal> {
        let resources = match resources {
            Some(resources) => match resources.capture_on_control(metadata_limit) {
                Ok(captured) => captured,
                Err(error) => {
                    return Err(ResidentPublicationRefusal {
                        kind: hex_session::service::ResidentPublicationFailure::Other,
                        playback,
                        error: format!("{error:?}"),
                    });
                }
            },
            None => {
                return Err(ResidentPublicationRefusal {
                    kind: hex_session::service::ResidentPublicationFailure::Other,
                    playback,
                    error: "missing resources".into(),
                });
            }
        };
        let candidate =
            DeviceResourceAdmission::unpublished_on_control(&playback, Some(&resources));
        let previous = self.resident_admission();
        if let Err(error) = admit(&[candidate, previous[0]]) {
            return Err(ResidentPublicationRefusal {
                kind: hex_session::service::ResidentPublicationFailure::Other,
                playback,
                error,
            });
        }
        self.active = Some((playback, resources, observation));
        self.next_audio_revision += 1;
        let audio_revision = self.next_audio_revision;
        self.readiness = ResidentPublicationReadiness::Applied {
            submission: ResidentRuntimeLoop {
                source_origin: ResidentSourceOrigin::Unknown,
                output_identity: 1,
                audio_revision,
                source_context: Some(metadata.context),
                section_loop: None,
            },
            publication_in_flight: false,
        };
        Ok(ResidentPublicationAcceptance::Submitted {
            output_identity: 1,
            audio_revision,
        })
    }

    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(SAMPLE_RATE)
    }

    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}

    fn poll(&mut self) {}

    fn close(&mut self) {
        self.active = None;
    }
}

fn wav_stereo_pcm16(sample_rate: u32, frames: usize, frequency_hz: f32) -> Vec<u8> {
    let data_bytes = (frames * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    let phase_step = std::f32::consts::TAU * frequency_hz / sample_rate as f32;
    for frame in 0..frames {
        let value = ((frame as f32 * phase_step).sin() * 12_000.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes.extend_from_slice(&(-value).to_le_bytes());
    }
    bytes
}

fn next_event(service: &Service) -> Result<Event, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        match service.try_event() {
            Ok(event) => return Ok(event),
            Err(TryRecvError::Disconnected) => return Err("service event channel closed".into()),
            Err(TryRecvError::Empty) if started.elapsed() < EVENT_TIMEOUT => thread::yield_now(),
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

fn run_ok(service: &Service, request: Request) -> Result<Event, Box<dyn Error>> {
    submit(service, request)?;
    let event = next_event(service)?;
    if let Some(error) = &event.error {
        return Err(format!("request failed: {error}").into());
    }
    Ok(event)
}

fn summarize(label: &str, samples_us: &mut [u128]) -> (u128, f64) {
    samples_us.sort_unstable();
    let n = samples_us.len();
    let p50 = samples_us[n / 2];
    let p95 = samples_us[(n * 95) / 100];
    let max = samples_us[n - 1];
    let mean = samples_us.iter().sum::<u128>() as f64 / n as f64;
    println!("  {label:<32} p50={p50:>6}us  mean={mean:>7.1}us  p95={p95:>6}us  max={max:>6}us");
    (p50, mean)
}

fn main() -> Result<(), Box<dyn Error>> {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "hex-trivial-edits-bench-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root)?;

    // Build real PCM WAV fixtures: 1.0s Sampler hit (48k stereo frames) + 2.0s audio stem (96k stereo frames).
    let mut sampler_paths: Vec<PathBuf> = Vec::with_capacity(8);
    for i in 0..8 {
        let path = root.join(format!("sampler_{i:02}.wav"));
        std::fs::write(
            &path,
            wav_stereo_pcm16(SAMPLE_RATE, 48_000, 110.0 + i as f32 * 55.0),
        )?;
        sampler_paths.push(path);
    }
    let mut audio_paths: Vec<PathBuf> = Vec::with_capacity(12);
    for i in 0..12 {
        let path = root.join(format!("stem_{i:02}.wav"));
        std::fs::write(
            &path,
            wav_stereo_pcm16(SAMPLE_RATE, 96_000, 220.0 + i as f32 * 30.0),
        )?;
        audio_paths.push(path);
    }

    let retained_budget = 512 * 1024 * 1024_usize;
    let mut service = Service::with_audio_sink_and_native_reservation(
        HISTORY_CAPACITY,
        64 * 1024 * 1024,
        Some(root.join("bench.hex-journal")),
        Box::new(BenchmarkSink::new()),
        ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 512,
                root_rows: 256,
                max_retained_bytes: retained_budget,
                max_construction_peak_bytes: retained_budget * 2,
            },
            max_scan_frames: 256_000,
        },
    )?;
    let init = next_event(&service)?;
    if let Some(error) = init.error {
        return Err(format!("initial service error: {error}").into());
    }
    let generation = 0_u64;

    let utility = DeviceTypeId::new("hex.utility")?;
    let eq8 = DeviceTypeId::new("hex.eq8")?;

    // 2 Return tracks with built-in effects.
    let mut return_ids = Vec::new();
    for idx in 0..2 {
        let ev = run_ok(
            &service,
            Request::Edit(Command::AddReturnTrack {
                name: format!("return {idx}"),
            }),
        )?;
        let Some(Created::ReturnTrack(ret)) = ev.created else {
            return Err("missing return id".into());
        };
        return_ids.push(ret);
    }

    let mut request_id = 1_u64;
    let mut sampler_tracks = Vec::new();
    // 8 Sampler tracks with loaded PCM samples + 16-note MIDI clips + utility + send.
    for (idx, path) in sampler_paths.iter().enumerate() {
        let added = run_ok(
            &service,
            Request::Edit(Command::AddMidiTrack {
                name: format!("sampler {idx:02}"),
            }),
        )?;
        let Some(Created::Track(track)) = added.created else {
            return Err("missing sampler track id".into());
        };
        let _loaded = run_ok(
            &service,
            Request::LoadFirstSampler {
                request_id,
                project_generation: generation,
                revision: added.state.revision,
                target: SamplerLoadTarget::Track {
                    track,
                    slot: SamplerLoadSlot::New(InstrumentLoadPlacement::First),
                },
                path: path.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )?;
        request_id += 1;
        let _fx = run_ok(
            &service,
            Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(utility),
                before: None,
            },
        )?;
        let placed = run_ok(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(16)?,
            }),
        )?;
        let Some(Created::Clip(clip)) = placed.created else {
            return Err("missing sampler clip".into());
        };
        for step in 0..16_u64 {
            let _ = run_ok(
                &service,
                Request::Edit(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_quarters(step)?,
                    duration: BeatTime::from_quarters(1)?,
                    pitch: 48 + (step % 12) as u8,
                    channel: 0,
                    velocity: 100,
                }),
            )?;
        }
        let _ = run_ok(
            &service,
            Request::Edit(Command::SetReturnSend {
                track,
                return_track: return_ids[0],
                gain: 0.25,
            }),
        )?;
        sampler_tracks.push(track);
    }

    // 12 Audio tracks with resident 2.0s stereo audio clips + EQ8 + send.
    let mut audio_tracks = Vec::new();
    for path in &audio_paths {
        let imported = run_ok(&service, Request::Import(path.clone()))?;
        let track = imported
            .state
            .project
            .tracks()
            .last()
            .ok_or("missing imported audio track")?
            .id();
        let _fx = run_ok(
            &service,
            Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(eq8),
                before: None,
            },
        )?;
        let _ = run_ok(
            &service,
            Request::Edit(Command::SetReturnSend {
                track,
                return_track: return_ids[0],
                gain: 0.2,
            }),
        )?;
        audio_tracks.push(track);
    }

    // 12 MIDI tracks with 32 notes each + utility + send.
    let mut midi_tracks = Vec::new();
    let mut target_note = None;
    for idx in 0..12 {
        let added = run_ok(
            &service,
            Request::Edit(Command::AddMidiTrack {
                name: format!("midi {idx:02}"),
            }),
        )?;
        let Some(Created::Track(track)) = added.created else {
            return Err("missing midi track".into());
        };
        let _fx = run_ok(
            &service,
            Request::AddNativeDevice {
                track,
                class: hex_engine::NativeDeviceClass::Effect,
                type_id: Box::new(utility),
                before: None,
            },
        )?;
        let placed = run_ok(
            &service,
            Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(32)?,
            }),
        )?;
        let Some(Created::Clip(clip)) = placed.created else {
            return Err("missing midi clip".into());
        };
        for step in 0..32_u64 {
            let ev = run_ok(
                &service,
                Request::Edit(Command::AddMidiNote {
                    clip,
                    start: BeatTime::from_quarters(step)?,
                    duration: BeatTime::from_quarters(1)?,
                    pitch: 60 + (step % 12) as u8,
                    channel: 0,
                    velocity: 100,
                }),
            )?;
            if idx == 0
                && step == 0
                && let Some(Created::Note(note)) = ev.created
            {
                target_note = Some(note);
            }
        }
        let _ = run_ok(
            &service,
            Request::Edit(Command::SetReturnSend {
                track,
                return_track: return_ids[1],
                gain: 0.15,
            }),
        )?;
        midi_tracks.push(track);
    }

    let target_note = target_note.ok_or("missing target note")?;
    let target_track = audio_tracks[0];
    let _ = &midi_tracks[11];

    println!(
        "32-track realistic fixture: 8 Sampler (48k stereo PCM), 12 Audio (96k stereo WAV + EQ8), 12 MIDI (32 notes + Utility), 2 Returns, 48000 Hz, recovery journal enabled"
    );
    println!(
        "Warm-up: {WARMUP_ITERATIONS} iterations, Timed: {TIMED_ITERATIONS} iterations per operation"
    );

    // 1. Baseline full-compile path on the exact same 32-track worker (SetLoopRegion changes
    //    loop_region, bypassing try_prepare_incremental and running the full PreparedDeviceResources
    //    + SignalGraph + AudioClipWaveform + PreparedSessionInventory compile path).
    for i in 0..WARMUP_ITERATIONS {
        black_box(run_ok(
            &service,
            Request::Edit(Command::SetLoopRegion {
                start: 0,
                end: 96_000 + (i as u64) * 1024,
                enabled: true,
            }),
        )?);
    }
    let mut full_compile_us = Vec::with_capacity(TIMED_ITERATIONS);
    for i in 0..TIMED_ITERATIONS {
        let started = Instant::now();
        let ev = run_ok(
            &service,
            Request::Edit(Command::SetLoopRegion {
                start: 0,
                end: 144_000 + (i as u64) * 1024,
                enabled: true,
            }),
        )?;
        full_compile_us.push(started.elapsed().as_micros());
        black_box(ev.state.revision);
    }

    // 2. Fast-path Mute / Solo toggles.
    for i in 0..WARMUP_ITERATIONS {
        black_box(run_ok(
            &service,
            Request::Edit(Command::Mute {
                track: target_track,
                muted: i % 2 == 0,
            }),
        )?);
    }
    let mut mute_solo_us = Vec::with_capacity(TIMED_ITERATIONS);
    for i in 0..TIMED_ITERATIONS {
        let cmd = if i % 2 == 0 {
            Command::Mute {
                track: target_track,
                muted: (i / 2) % 2 == 0,
            }
        } else {
            Command::ToggleSolo {
                track: sampler_tracks[0],
                exclusive: false,
            }
        };
        let started = Instant::now();
        let ev = run_ok(&service, Request::Edit(cmd))?;
        mute_solo_us.push(started.elapsed().as_micros());
        black_box(ev.state.revision);
    }

    // 3. Fast-path Track Gain continuous drag ticks via GestureClient.
    let mut client = GestureClient::default();
    let target = GestureTarget::TrackGain {
        track: target_track,
    };
    let _key = client.begin(0, target, ParameterValue::Float(1.0))?;
    let begin_req = client.next_request().ok_or("missing begin")?;
    submit(&service, Request::device_gesture(begin_req))?;
    client.mark_submitted();
    let begin_ev = next_event(&service)?;
    let begin_ack = begin_ev.gesture_ack.as_ref().ok_or("missing begin ack")?;
    if !begin_ack.accepted
        || begin_ack.phase != GesturePhase::Begin
        || !client.acknowledge(begin_ack)
    {
        return Err("begin ack rejected".into());
    }
    for i in 0..WARMUP_ITERATIONS {
        let gain = 0.5 + (i as f32) * 0.01;
        client.change(ParameterValue::Float(gain))?;
        let req = client.next_request().ok_or("missing warmup change")?;
        submit(&service, Request::device_gesture(req))?;
        client.mark_submitted();
        let ev = next_event(&service)?;
        let ack = ev.gesture_ack.as_ref().ok_or("missing warmup ack")?;
        if !ack.accepted || !client.acknowledge(ack) {
            return Err("warmup change rejected".into());
        }
    }
    let mut gain_drag_us = Vec::with_capacity(TIMED_ITERATIONS);
    for i in 0..TIMED_ITERATIONS {
        let gain = 0.25 + (i as f32) * 0.005;
        let started = Instant::now();
        client.change(ParameterValue::Float(gain))?;
        let req = client.next_request().ok_or("missing change")?;
        submit(&service, Request::device_gesture(req))?;
        client.mark_submitted();
        let ev = next_event(&service)?;
        let ack = ev.gesture_ack.as_ref().ok_or("missing change ack")?;
        if !ack.accepted || !client.acknowledge(ack) {
            return Err("change ack rejected".into());
        }
        gain_drag_us.push(started.elapsed().as_micros());
        black_box(ev.device_preview.is_some());
    }
    client.finish(ParameterValue::Float(0.75))?;
    if let Some(req) = client.next_request() {
        submit(&service, Request::device_gesture(req))?;
        client.mark_submitted();
        let _ = next_event(&service)?;
    }

    // 4. Fast-path MIDI note moves (UpdateMidiNote).
    for i in 0..WARMUP_ITERATIONS {
        let pitch = 60 + (i % 7) as u8;
        black_box(run_ok(
            &service,
            Request::Edit(Command::UpdateMidiNote {
                note: target_note,
                start: BeatTime::from_quarters((i % 2) as u64)?,
                duration: BeatTime::from_quarters(1)?,
                pitch,
                channel: 0,
                velocity: 100,
            }),
        )?);
    }
    let mut midi_move_us = Vec::with_capacity(TIMED_ITERATIONS);
    for i in 0..TIMED_ITERATIONS {
        let pitch = 60 + ((i + 1) % 12) as u8;
        let started = Instant::now();
        let ev = run_ok(
            &service,
            Request::Edit(Command::UpdateMidiNote {
                note: target_note,
                start: BeatTime::from_quarters((i % 2) as u64)?,
                duration: BeatTime::from_quarters(1)?,
                pitch,
                channel: 0,
                velocity: 100,
            }),
        )?;
        midi_move_us.push(started.elapsed().as_micros());
        black_box(ev.state.revision);
    }

    println!("\nResults (worker roundtrip wall time):");
    let (full_p50, full_mean) = summarize("full-compile baseline", &mut full_compile_us);
    let (mute_p50, mute_mean) = summarize("Mute / Solo (fast-path)", &mut mute_solo_us);
    let (gain_p50, gain_mean) = summarize("Track Gain drag tick (fast-path)", &mut gain_drag_us);
    let (midi_p50, midi_mean) = summarize("MIDI note move (fast-path)", &mut midi_move_us);

    println!("\nSpeedup vs full-compile baseline:");
    println!(
        "  Mute / Solo:          p50={:.1}x  mean={:.1}x",
        full_p50 as f64 / mute_p50.max(1) as f64,
        full_mean / mute_mean.max(0.1)
    );
    println!(
        "  Track Gain drag tick: p50={:.1}x  mean={:.1}x",
        full_p50 as f64 / gain_p50.max(1) as f64,
        full_mean / gain_mean.max(0.1)
    );
    println!(
        "  MIDI note move:       p50={:.1}x  mean={:.1}x",
        full_p50 as f64 / midi_p50.max(1) as f64,
        full_mean / midi_mean.max(0.1)
    );

    let _ = service.shutdown(Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
