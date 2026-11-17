//! Worker-constructed verified gain metadata. The canonical source remains the
//! sole source-fact/context authority; an overlay never mints canonical freshness.
//! This owns Project metadata only, not a native graph, reader, page or lease.
use super::*;
use crate::device_gesture::{GestureKey, GesturePhase, GestureTarget, ParameterValue};

pub struct VerifiedTrackGainSource {
    canonical: Arc<CanonicalAudioSourceSnapshot>,
    preview: Option<(DevicePreviewIdentity, Arc<Project>)>,
    terminal: Option<(GestureKey, u64)>,
}
impl VerifiedTrackGainSource {
    pub(super) fn capture(
        event: &Event,
        previous_gain: Option<(GestureKey, u64)>,
    ) -> Result<Option<Self>, SessionError> {
        let Some(canonical) = event
            .state
            .canonical_audio_source
            .as_ref()
            .filter(|source| source.audio_mode() == DocumentAudioMode::VerifiedStreaming)
        else {
            return Ok(None);
        };
        if let Some(active) = event.active_device_gesture
            && let Some(track) = active.target.track_gain_anchor()
        {
            let Some(preview) = event.device_preview.as_ref() else {
                return Ok(None);
            };
            let identity = preview.identity;
            let gain = preview.state.project.track(track).map(|track| track.gain());
            let group_correlated = match (active.target, active.track_gains, active.value) {
                (
                    GestureTarget::TrackGains { track },
                    Some(group),
                    ParameterValue::Float(value),
                ) => {
                    group.anchor() == track
                        && group.validate_current(&event.state.project).is_ok()
                        && group.values(value).is_ok_and(|values| {
                            values.into_iter().all(|(id, gain)| {
                                preview
                                    .state
                                    .project
                                    .track(id)
                                    .is_some_and(|track| track.gain() == gain)
                            })
                        })
                }
                (GestureTarget::TrackGain { .. }, None, _) => true,
                _ => false,
            };
            if !group_correlated
                || identity.project_generation != canonical.context().project_generation
                || identity.key.generation != canonical.context().project_generation
                || identity.key.token == 0
                || preview.state.revision != event.state.revision
                || preview.state.canonical_audio_source.is_some()
                || identity.canonical_revision != event.state.revision
                || event.state.audio_authoring_revision(canonical.context())
                    != Some(event.state.revision)
                || identity.key != active.key
                || identity.sequence != active.sequence
                || !matches!(active.value, ParameterValue::Float(value) if value.is_finite() && (0.0..=2.0).contains(&value) && gain == Some(value))
            {
                return Err(SessionError::Invalid(
                    "verified gain preview correlation changed",
                ));
            }
            return Ok(Some(Self {
                canonical: Arc::clone(canonical),
                preview: Some((identity, Arc::clone(&preview.state.project))),
                terminal: None,
            }));
        }
        // A refused/future Cancel is not a terminal proof. The prior delivered
        // actor snapshot establishes that this worker actually owned the key.
        let Some((owned_key, owned_sequence)) = previous_gain else {
            return Ok(None);
        };
        let Some(ack) = event.gesture_ack.as_ref().filter(|ack| {
            matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel)
                && ack.key == owned_key
                && ack.key.token != 0
                && ack.key.generation == canonical.context().project_generation
                && !event
                    .active_device_gesture
                    .is_some_and(|active| active.key == ack.key)
        }) else {
            return Ok(None);
        };
        Ok(Some(Self {
            canonical: Arc::clone(canonical),
            preview: None,
            terminal: Some((ack.key, owned_sequence)),
        }))
    }
    pub fn canonical(&self) -> &Arc<CanonicalAudioSourceSnapshot> {
        &self.canonical
    }
    pub fn preview_identity(&self) -> Option<DevicePreviewIdentity> {
        self.preview.as_ref().map(|(identity, _)| *identity)
    }
    pub fn terminal_key(&self) -> Option<GestureKey> {
        self.terminal.map(|(key, _)| key)
    }
    pub fn terminal_sequence(&self) -> Option<u64> {
        self.terminal.map(|(_, sequence)| sequence)
    }
    fn project(&self) -> &Project {
        self.preview.as_ref().map_or_else(
            || self.canonical.project_metadata(),
            |(_, project)| project.as_ref(),
        )
    }
    /// Same finite constructor/class admission as the canonical source. A gain
    /// overlay cannot add a source, native device class or ownership capability.
    pub fn candidate_reservation_quote(
        &self,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        self.canonical
            .candidate_reservation_quote_for(self.project())
    }
    /// Borrow host effect facts while retaining the canonical source and context.
    pub fn reservation_quote_with_effects(
        &self,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<hex_engine::AudioCandidateReservationQuote, hex_engine::AudioStorageError> {
        self.canonical
            .candidate_reservation_quote_for_effects(self.project(), Some(effects))
    }
    /// Construct the same overlay using host-borrowed effect facts.
    pub fn prepare_candidate_with_effects_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
        effects: &hex_engine::AudioCandidateEffects<'_, '_>,
    ) -> Result<(), hex_engine::AudioStorageError> {
        effects.prepare(
            destination,
            self.project(),
            catalog,
            self.canonical.context(),
            None,
        )
    }
    /// Compile in the existing reserved host cell, retaining the actual canonical
    /// context and catalog. No resident fallback or source-owner extraction.
    pub fn prepare_candidate_on_control(
        &self,
        catalog: u64,
        destination: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        destination.prepare_on_control(self.project(), catalog, self.canonical.context())
    }
}

#[cfg(test)]
mod tests {
    mod group;
    use super::*;
    use std::{
        sync::{Mutex, mpsc},
        time::Duration,
    };
    fn next(service: &Service) -> Event {
        service
            .events
            .recv_timeout(crate::test_wait::HANG_BOUND)
            .unwrap()
    }
    fn send(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        let event = next(service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }
    fn fixture() -> (Service, Event, Event, Event, GestureKey) {
        let service = Service::new(48_000, 16, 0).unwrap();
        let initial = next(&service);
        send(
            &service,
            Request::NewWithAudioMode {
                mode: DocumentAudioMode::VerifiedStreaming,
                reservation: Some(crate::SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits::new(
                        hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                        1024 * 1024,
                    ),
                    max_retained_sources: 1,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 1024 * 1024,
                }),
            },
        );
        let before = send(
            &service,
            Request::Edit(Command::AddTrack {
                name: "gain".into(),
            }),
        );
        let track = before.state.project.tracks()[0].id();
        let key = GestureKey {
            generation: before
                .state
                .canonical_audio_source
                .as_ref()
                .unwrap()
                .context()
                .project_generation,
            token: 1,
        };
        let began = send(
            &service,
            Request::DeviceGesture(crate::device_gesture::GestureRequest::Begin {
                key,
                target: GestureTarget::TrackGain { track },
            }),
        );
        assert!(began.gesture_ack.as_ref().unwrap().accepted);
        let changed = send(
            &service,
            Request::DeviceGesture(crate::device_gesture::GestureRequest::Change {
                key,
                sequence: 1,
                value: ParameterValue::Float(0.25),
            }),
        );
        let cancelled = send(
            &service,
            Request::DeviceGesture(crate::device_gesture::GestureRequest::Cancel { key }),
        );
        (service, initial, changed, cancelled, key)
    }
    #[test]
    fn typed_projection_preserves_actual_canonical_source_and_refuses_changed_identity() {
        let (service, _initial, mut changed, cancelled, key) = fixture();
        let source = VerifiedTrackGainSource::capture(&changed, None)
            .unwrap()
            .unwrap();
        let track = changed.state.project.tracks()[0].id();
        assert!(Arc::ptr_eq(
            source.canonical(),
            changed.state.canonical_audio_source.as_ref().unwrap()
        ));
        assert_eq!(source.project().track(track).unwrap().gain(), 0.25);
        assert_eq!(changed.state.project.track(track).unwrap().gain(), 1.0);
        assert_eq!(source.preview_identity().unwrap().key, key);
        changed.device_preview.as_mut().unwrap().identity.sequence += 1;
        assert!(VerifiedTrackGainSource::capture(&changed, None).is_err());
        let terminal = VerifiedTrackGainSource::capture(&cancelled, Some((key, 1)))
            .unwrap()
            .unwrap();
        assert_eq!(terminal.terminal_key(), Some(key));
        assert!(terminal.preview_identity().is_none());
        assert_eq!(terminal.project(), cancelled.state.project.as_ref());
        assert_eq!(terminal.terminal_sequence(), Some(1));
        let future = send(
            &service,
            Request::DeviceGesture(crate::device_gesture::GestureRequest::Cancel {
                key: GestureKey { token: 99, ..key },
            }),
        );
        assert!(!future.gesture_ack.as_ref().unwrap().accepted);
        assert!(
            VerifiedTrackGainSource::capture(&future, None)
                .unwrap()
                .is_none()
        );
        assert!(
            VerifiedTrackGainSource::capture(&future, Some((key, 1)))
                .unwrap()
                .is_none()
        );
    }

    #[derive(Default)]
    struct Observed {
        canonical: Option<Arc<CanonicalAudioSourceSnapshot>>,
        sources: Vec<Arc<VerifiedTrackGainSource>>,
    }
    struct Sink(Arc<Mutex<Observed>>);
    impl WorkerAudioSink for Sink {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Ok(48_000)
        }
        fn canonical_changed(&mut self, source: Arc<CanonicalAudioSourceSnapshot>) {
            self.0.lock().unwrap().canonical = Some(source);
        }
        fn verified_track_gain_source(&mut self, source: Arc<VerifiedTrackGainSource>) {
            let mut observed = self.0.lock().unwrap();
            assert!(Arc::ptr_eq(
                observed.canonical.as_ref().unwrap(),
                source.canonical()
            ));
            assert!(observed.sources.len() < 2);
            observed.sources.push(source);
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
    }
    fn await_count(observed: &Mutex<Observed>, count: usize) {
        let until = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
        while observed.lock().unwrap().sources.len() < count {
            assert!(
                std::time::Instant::now() < until,
                "source notification blocked behind UI event"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn worker_delivers_preview_and_terminal_before_full_ui_queue() {
        let (_service, initial, changed, cancelled, key) = fixture();
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(initial).unwrap();
        let observed = Arc::new(Mutex::new(Observed::default()));
        let mut worker = WorkerEvents::new(
            sender,
            Arc::new(AtomicBool::new(false)),
            Some(Box::new(Sink(Arc::clone(&observed)))),
        );
        let thread = std::thread::spawn(move || {
            assert!(worker.send_with_control_poll(changed, |_| {}));
            worker
        });
        await_count(&observed, 1);
        assert_eq!(
            observed.lock().unwrap().sources[0]
                .preview_identity()
                .unwrap()
                .key,
            key
        );
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut worker = thread.join().unwrap();
        // The Change now fills the only UI row while its terminal still reaches audio.
        let thread = std::thread::spawn(move || {
            assert!(worker.send_with_control_poll(cancelled, |_| {}));
            worker
        });
        await_count(&observed, 2);
        assert_eq!(
            observed.lock().unwrap().sources[1].terminal_key(),
            Some(key)
        );
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let worker = thread.join().unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(worker);
    }
}
