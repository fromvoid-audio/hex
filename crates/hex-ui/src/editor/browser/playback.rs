//! Resident and windowed browser sample audition playback and polling.
use super::*;

impl EditorView {
    pub(in crate::editor) fn cancel_browser_sample_playback(&mut self) {
        self.browser.playback_generation = self.browser.playback_generation.saturating_add(1);
        self.cancel_browser_playback_preparation();
        self.browser.playback_pending = None;
        self.browser.playback_phase = Phase::Idle;
        let output = self.browser.playback_output.take();
        self.browser.playback_path = None;
        self.browser.playback_project_sample = None;
        self.browser.playback_fraction = 0.0;
        if let Some((identity, generation)) = output
            && let Some(audio) = self.audio.as_mut().filter(|audio| {
                audio.control.output_identity() == Some(identity)
                    && audio
                        .control
                        .preview_snapshot()
                        .is_some_and(|snapshot| snapshot.generation == generation)
            })
        {
            audio.control.preview_cancel();
        }
    }

    /// Stop the in-flight audition preparation within bounded work; its
    /// result is already stale because the playback generation moved on.
    pub(super) fn cancel_browser_playback_preparation(&self) {
        if let Some(cancel) = &self.browser.playback_in_flight {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// A completed sample drop (admitted into a device, track, Arrangement,
    /// Session or new track) ends that browser interaction, so it stops the
    /// active preview through the ordinary cancellation. A cancelled or refused
    /// drop leaves the preview playing until the user moves on.
    pub(in crate::editor) fn stop_browser_preview_for_drop(&mut self) {
        if self.browser.playback_output.is_some() || self.browser.playback_pending.is_some() {
            self.cancel_browser_sample_playback();
        }
    }

    pub(super) fn play_browser_sample(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.play_browser_sample_from(path, 0.0, cx);
    }

    pub(super) fn play_browser_sample_from(
        &mut self,
        path: PathBuf,
        fraction: f64,
        cx: &mut Context<Self>,
    ) {
        // Previously painted controls can retain a path across source validation.
        if self.browser_place_unavailable()
            || self.browser.section == BrowserSection::ProjectSamples
        {
            return;
        }
        self.play_browser_audio(BrowserPlaybackSource::File(path), fraction, cx);
    }

    pub(super) fn play_browser_project_sample_from(
        &mut self,
        key: &BrowserItemKey,
        fraction: f64,
        cx: &mut Context<Self>,
    ) {
        if self.browser.section != BrowserSection::ProjectSamples {
            return;
        }
        let source = self.state.as_ref().and_then(|state| {
            let BrowserItemKey::ProjectSample(generation, revision, asset) = key else {
                return None;
            };
            if !self.project_sample_current(key) {
                return None;
            }
            let snapshot = state.canonical_audio_source.as_ref()?;
            let context = snapshot.context();
            if context.project_generation != *generation || context.editor_revision != *revision {
                return None;
            }
            match snapshot.sources().get(asset)? {
                hex_session::AudioSourceBinding::Resident(audio) => Some(audio.clone()),
                hex_session::AudioSourceBinding::VerifiedFile(_) => None,
            }
        });
        let Some(source) = source else {
            self.cancel_browser_sample_playback();
            self.error =
                Some("sample preview unavailable · current resident source required".into());
            return;
        };
        self.play_browser_audio(
            BrowserPlaybackSource::ProjectSample(key.clone(), source),
            fraction,
            cx,
        );
    }

    pub(super) fn play_browser_audio(
        &mut self,
        source: BrowserPlaybackSource,
        fraction: f64,
        cx: &mut Context<Self>,
    ) {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.browser.playback_output.is_some() {
            self.cancel_browser_sample_playback();
        }
        self.browser.playback_phase = Phase::Unavailable;
        self.browser.playback_fraction = fraction as f32;
        self.status = "sample preview unavailable".into();
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error = Some("sample preview is unavailable in verified streaming mode".into());
            return;
        }
        let Some(generation) = self.browser.playback_generation.checked_add(1) else {
            self.error = Some("sample preview request counter exhausted".into());
            return;
        };
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.starts_with("sample preview"))
        {
            self.error = None;
        }
        self.browser.playback_generation = generation;
        self.cancel_browser_playback_preparation();
        match &source {
            BrowserPlaybackSource::File(path) => {
                self.browser.playback_path = Some(path.clone());
                self.browser.playback_project_sample = None;
            }
            BrowserPlaybackSource::ProjectSample(key, _) => {
                self.browser.playback_path = None;
                self.browser.playback_project_sample = Some(key.clone());
            }
        }
        self.browser.playback_pending = None;
        let Some(audio) = self.audio.as_mut() else {
            self.error = Some("sample preview unavailable · no audio output".into());
            return;
        };
        let output_generation = match audio
            .control
            .preview_set_gain(self.browser.audition_gain)
            .and_then(|()| audio.control.preview_request())
        {
            Ok(token) => token,
            Err(error) => {
                self.error = Some(format!("sample preview failed · {error}").into());
                return;
            }
        };
        let Some(output_identity) = audio.control.output_identity() else {
            self.error = Some("sample preview unavailable · audio output closed".into());
            return;
        };
        let request = PlaybackPreviewRequest {
            generation,
            output_generation,
            output_identity,
            source,
            sample_rate: audio.sample_rate,
            start_fraction: fraction,
        };
        self.browser.playback_phase = Phase::Preparing;
        self.browser.playback_output = Some((output_identity, output_generation));
        self.status = "sample preview preparing".into();
        if self.browser.playback_in_flight.is_some() {
            self.browser.playback_pending = Some(request);
        } else {
            self.start_browser_sample_playback(request, cx);
        }
    }

    pub(super) fn start_browser_sample_playback(
        &mut self,
        request: PlaybackPreviewRequest,
        cx: &mut Context<Self>,
    ) {
        if let BrowserPlaybackSource::File(path) = &request.source {
            self.start_windowed_browser_sample_playback(request.clone(), path.clone(), cx);
        } else {
            self.start_resident_browser_sample_playback(request, cx);
        }
    }

    pub(super) fn start_resident_browser_sample_playback(
        &mut self,
        request: PlaybackPreviewRequest,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.browser.playback_in_flight = Some(Arc::clone(&cancel));
        let sources = self.browser.preview_sources.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let source = request.source.clone();
            let sample_rate = request.sample_rate;
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    match source {
                        BrowserPlaybackSource::File(path) => {
                            sources.prepare_playback(path, sample_rate, &cancel)
                        }
                        BrowserPlaybackSource::ProjectSample(_, audio) => {
                            PreviewSources::prepare_resident_playback(&audio, sample_rate, &cancel)
                        }
                    }
                    .map_err(|failure| failure.to_string())
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.playback_in_flight = None;
                let source_current = match &request.source {
                    BrowserPlaybackSource::File(_) => true,
                    BrowserPlaybackSource::ProjectSample(key, _) => {
                        view.project_sample_current(key)
                    }
                };
                if request.generation == view.browser.playback_generation && !source_current {
                    view.cancel_browser_sample_playback();
                }
                if request.generation == view.browser.playback_generation
                    && source_current
                    && let Some(audio) = view.audio.as_mut().filter(|audio| {
                        audio.control.output_identity() == Some(request.output_identity)
                            && audio.sample_rate == request.sample_rate
                    })
                {
                    let admitted = result.and_then(|pcm| {
                        let frame = audition_geometry::frame_at_fraction(
                            pcm.frames().len(),
                            request.start_fraction,
                        );
                        audio
                            .control
                            .preview_publish_from(request.output_generation, pcm, frame)
                            .map_err(|error| error.to_string())
                    });
                    match admitted {
                        Ok(()) => {
                            if view
                                .error
                                .as_ref()
                                .is_some_and(|error| error.starts_with("sample preview"))
                            {
                                view.error = None;
                            }
                            // Callback acknowledgement, not publication, marks playback.
                        }
                        Err(error) => {
                            audio.control.preview_cancel();
                            view.browser.playback_output = None;
                            view.browser.playback_phase = Phase::Unavailable;
                            view.status = "sample preview failed".into();
                            view.error = Some(format!("sample preview failed · {error}").into())
                        }
                    }
                }
                if let Some(pending) = view.browser.playback_pending.take() {
                    view.start_browser_sample_playback(pending, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn start_windowed_browser_sample_playback(
        &mut self,
        request: PlaybackPreviewRequest,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.browser.playback_in_flight = Some(Arc::clone(&cancel));
        #[cfg(test)]
        let test_environment = Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            #[cfg(test)]
            let _test_environment = test_environment;
            let open_cancel = Arc::clone(&cancel);
            let rate = request.sample_rate;
            let opened = cx
                .background_executor()
                .spawn(async move { PreviewWindowSource::open(path, rate, &open_cancel) })
                .await;
            let mut source = match opened {
                Ok(source) => source,
                Err(PreviewFailure::Cancelled) => {
                    let _ = this.update(cx, |view, cx| {
                        view.finish_browser_window_preparation(None, &request, cx)
                    });
                    return;
                }
                Err(PreviewFailure::SourceChanged) => {
                    let _ = this.update(cx, |view, cx| {
                        view.finish_browser_window_preparation(
                            Some(PreviewFailure::SourceChanged.to_string()),
                            &request,
                            cx,
                        )
                    });
                    return;
                }
                Err(_) => {
                    let _ = this.update(cx, |view, cx| {
                        view.browser.playback_in_flight = None;
                        if request.generation == view.browser.playback_generation
                            && !cancel.load(Ordering::Relaxed)
                        {
                            view.start_resident_browser_sample_playback(request, cx);
                        } else if let Some(pending) = view.browser.playback_pending.take() {
                            view.start_browser_sample_playback(pending, cx);
                        }
                    });
                    return;
                }
            };
            let total = match usize::try_from(source.output_frames()) {
                Ok(total) => total,
                Err(_) => {
                    let _ = this.update(cx, |view, cx| {
                        view.finish_browser_window_preparation(
                            Some("sample preview length is too large".into()),
                            &request,
                            cx,
                        )
                    });
                    return;
                }
            };
            let mut position = audition_geometry::frame_at_fraction(total, request.start_fraction);
            let started = this.update(cx, |view, _| {
                if cancel.load(Ordering::Relaxed)
                    || request.generation != view.browser.playback_generation
                {
                    return Ok(false);
                }
                let Some(audio) = view.audio.as_mut().filter(|audio| {
                    audio.control.output_identity() == Some(request.output_identity)
                        && audio.sample_rate == request.sample_rate
                        && audio.control.preview_snapshot().is_some_and(|snapshot| {
                            snapshot.generation == request.output_generation
                        })
                }) else {
                    return Ok(false);
                };
                audio
                    .control
                    .preview_begin_windows(request.output_generation, position, total)
                    .map(|()| true)
                    .map_err(|error| error.to_string())
            });
            match started {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) | Err(_) => {
                    let _ = this.update(cx, |view, cx| {
                        view.finish_browser_window_preparation(None, &request, cx)
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let _ = this.update(cx, |view, cx| {
                        view.finish_browser_window_preparation(Some(error), &request, cx)
                    });
                    return;
                }
            }
            let mut failure = None;
            while position < total && !cancel.load(Ordering::Relaxed) {
                let count = (total - position).min(BROWSER_PREVIEW_WINDOW_FRAMES);
                let read_cancel = Arc::clone(&cancel);
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        let result = source.read_window(position as u64, count, &read_cancel);
                        (source, result)
                    })
                    .await;
                source = read.0;
                let frames = match read.1 {
                    Ok(frames) => frames,
                    Err(PreviewFailure::Cancelled) => break,
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                };
                let mut window = match hex_engine::Audio::new(rate, frames) {
                    Ok(window) => window,
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                };
                loop {
                    let delivery = this.update(cx, |view, _| {
                        if cancel.load(Ordering::Relaxed)
                            || request.generation != view.browser.playback_generation
                        {
                            return WindowDelivery::Stale;
                        }
                        let Some(audio) = view.audio.as_mut().filter(|audio| {
                            audio.control.output_identity() == Some(request.output_identity)
                                && audio.sample_rate == request.sample_rate
                                && audio.control.preview_snapshot().is_some_and(|snapshot| {
                                    snapshot.generation == request.output_generation
                                })
                        }) else {
                            return WindowDelivery::Stale;
                        };
                        audio.control.reclaim_preview();
                        match audio.control.preview_publish_window(
                            request.output_generation,
                            position,
                            window,
                        ) {
                            Ok(()) => WindowDelivery::Accepted,
                            Err(refusal)
                                if refusal.reason
                                    == hex_audio::sample_preview::WindowRefusalReason::Full =>
                            {
                                WindowDelivery::Full(refusal.audio)
                            }
                            Err(refusal) => WindowDelivery::Failed(refusal.to_string()),
                        }
                    });
                    match delivery {
                        Ok(WindowDelivery::Accepted) => {
                            position += count;
                            break;
                        }
                        Ok(WindowDelivery::Full(returned)) => {
                            window = returned;
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        Ok(WindowDelivery::Failed(error)) => {
                            failure = Some(error);
                            break;
                        }
                        Ok(WindowDelivery::Stale) | Err(_) => {
                            cancel.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            let _ = this.update(cx, |view, cx| {
                view.finish_browser_window_preparation(failure, &request, cx)
            });
        })
        .detach();
    }

    pub(super) fn finish_browser_window_preparation(
        &mut self,
        failure: Option<String>,
        request: &PlaybackPreviewRequest,
        cx: &mut Context<Self>,
    ) {
        self.browser.playback_in_flight = None;
        if request.generation == self.browser.playback_generation
            && let Some(error) = failure
            && let Some(audio) = self.audio.as_mut().filter(|audio| {
                audio.control.output_identity() == Some(request.output_identity)
                    && audio
                        .control
                        .preview_snapshot()
                        .is_some_and(|snapshot| snapshot.generation == request.output_generation)
            })
        {
            audio.control.preview_cancel();
            self.browser.playback_output = None;
            self.browser.playback_phase = Phase::Unavailable;
            self.status = "sample preview failed".into();
            self.error = Some(format!("sample preview failed · {error}").into());
        }
        if let Some(pending) = self.browser.playback_pending.take() {
            self.start_browser_sample_playback(pending, cx);
        }
        cx.notify();
    }

    pub(in crate::editor) fn browser_preview_selected(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.browser.results_focused || self.browser.section == BrowserSection::Instruments {
            return;
        }
        if self.browser.section == BrowserSection::ProjectSamples {
            if let Some(key) = self.browser_selected_item(cx) {
                self.play_browser_project_sample_from(&key, 0.0, cx);
            }
        } else if let Some(entry) = self.browser_selected_entry(cx) {
            match entry.kind {
                EntryKind::WaveAudio => self.play_browser_sample(entry.path, cx),
                EntryKind::Directory => self.expand_browser_folder(entry.path, cx),
                EntryKind::Project => {}
            }
        }
        cx.notify();
    }

    pub(in crate::editor) fn poll_browser_sample_playback(&mut self) -> bool {
        let Some((identity, _)) = self.browser.playback_output else {
            return false;
        };
        if self
            .browser
            .playback_project_sample
            .as_ref()
            .is_some_and(|key| !self.project_sample_current(key))
        {
            self.cancel_browser_sample_playback();
            return true;
        }
        let snapshot = self
            .audio
            .as_ref()
            .filter(|audio| audio.control.output_identity() == Some(identity))
            .and_then(|audio| audio.control.preview_snapshot());
        self.accept_browser_sample_snapshot(snapshot)
    }

    pub(super) fn accept_browser_sample_snapshot(
        &mut self,
        snapshot: Option<hex_audio::sample_preview::Snapshot>,
    ) -> bool {
        let Some((_, generation)) = self.browser.playback_output else {
            return false;
        };
        let mut progress_changed = false;
        let phase = snapshot.map_or(Phase::Unavailable, |snapshot| {
            if snapshot.generation == generation {
                if snapshot.frames > 0
                    && matches!(
                        snapshot.phase,
                        Phase::Preparing | Phase::Playing | Phase::Finished
                    )
                {
                    let fraction =
                        (snapshot.frame as f64 / snapshot.frames as f64).clamp(0.0, 1.0) as f32;
                    let width = self
                        .browser
                        .waveform_bounds
                        .get()
                        .map_or(1.0, |bounds| f32::from(bounds.size.width).max(1.0));
                    progress_changed = (fraction * width).floor()
                        != (self.browser.playback_fraction * width).floor();
                    self.browser.playback_fraction = fraction;
                }
                snapshot.phase
            } else if snapshot.generation == 0 && snapshot.phase == Phase::Idle {
                Phase::Idle
            } else {
                Phase::Unavailable
            }
        });
        self.accept_browser_sample_phase(phase) || progress_changed
    }

    pub(super) fn accept_browser_sample_phase(&mut self, phase: Phase) -> bool {
        if self.browser.playback_phase == phase {
            return false;
        }
        self.browser.playback_phase = phase;
        self.status = match phase {
            Phase::Idle => "sample preview stopped",
            Phase::Preparing => "sample preview preparing",
            Phase::Playing => "sample preview playing",
            Phase::Finished => "sample preview finished",
            Phase::Unavailable => "sample preview unavailable",
        }
        .into();
        if matches!(phase, Phase::Finished | Phase::Idle) {
            self.browser.playback_output = None;
        } else if phase == Phase::Unavailable {
            self.cancel_browser_sample_playback();
            self.browser.playback_phase = Phase::Unavailable;
            self.error = Some("sample preview unavailable · audio output stopped".into());
        }
        true
    }

    pub(super) fn browser_selection_audition(
        &mut self,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.browser.playback_path != path {
            if self.browser.playback_output.is_some() {
                self.cancel_browser_sample_playback();
                self.status = "sample preview stopped".into();
            } else {
                self.browser.playback_path = None;
                self.browser.playback_fraction = 0.0;
                self.browser.playback_phase = Phase::Idle;
            }
        }
        if self.browser.auto_audition
            && let Some(path) = path
        {
            self.play_browser_sample(path, cx);
        }
    }
}
