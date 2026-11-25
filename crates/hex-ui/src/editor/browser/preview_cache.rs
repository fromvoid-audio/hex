//! Browser audio preview slot cache, bounded eviction, and background metadata loading.
use super::*;

impl EditorView {
    pub(super) fn browser_preview_cell(&mut self, path: PathBuf, sample_rate: u32) -> PreviewSlot {
        let key = PreviewKey { path, sample_rate };
        if let Some(metadata) = self.browser.preview_cache.get(&key) {
            return metadata.clone();
        }
        self.evict_browser_preview_slot();
        let slot = PreviewSlot::default();
        self.browser.preview_cache.insert(key, slot.clone());
        slot
    }

    pub(super) fn evict_browser_preview_slot(&mut self) {
        if self.browser.preview_cache.len() < PREVIEW_CACHE_CAPACITY {
            return;
        }
        let protected = self
            .browser
            .preview_in_flight
            .iter()
            .chain(self.browser.preview_pending.iter())
            .map(|request| &request.key)
            .chain(self.browser.preview_selected.iter());
        if let Some(evicted) = self
            .browser
            .preview_cache
            .keys()
            .find(|candidate| !protected.clone().any(|key| key == *candidate))
            .cloned()
        {
            self.browser.preview_cache.remove(&evicted);
        }
    }

    pub(super) fn request_browser_audio_preview(
        &mut self,
        path: PathBuf,
        sample_rate: u32,
        cx: &mut Context<Self>,
    ) -> Rc<RefCell<Option<Rc<AudioPreview>>>> {
        let slot = self.browser_preview_cell(path.clone(), sample_rate);
        if slot.metadata.borrow().is_some() || slot.error.borrow().is_some() {
            return slot.metadata;
        }
        self.queue_browser_audio_preview(PreviewKey { path, sample_rate }, slot.clone(), cx);
        slot.metadata
    }

    pub(super) fn request_browser_audio_preview_for_drag(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        cx: &mut Context<Self>,
    ) {
        let key = PreviewKey {
            path: drag.path.clone(),
            sample_rate: drag.sample_rate,
        };
        let slot = PreviewSlot {
            metadata: drag.metadata.clone(),
            error: drag.metadata_error.clone(),
        };
        if !self.browser.preview_cache.contains_key(&key) {
            self.evict_browser_preview_slot();
        }
        self.browser.preview_cache.insert(key.clone(), slot.clone());
        *slot.metadata.borrow_mut() = None;
        *slot.error.borrow_mut() = None;
        self.queue_browser_audio_preview(key, slot, cx);
    }

    pub(super) fn queue_browser_audio_preview(
        &mut self,
        key: PreviewKey,
        slot: PreviewSlot,
        cx: &mut Context<Self>,
    ) {
        if slot.metadata.borrow().is_some() {
            return;
        }
        *slot.error.borrow_mut() = None;
        let request = PreviewRequest {
            key,
            slot: slot.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if let Some(in_flight) = self
            .browser
            .preview_in_flight
            .as_ref()
            .filter(|in_flight| in_flight.key == request.key)
        {
            if Rc::ptr_eq(&in_flight.slot.metadata, &request.slot.metadata) {
                self.browser.preview_pending = None;
            } else {
                self.browser.preview_pending = Some(request);
            }
            return;
        }
        if self
            .browser
            .preview_pending
            .as_ref()
            .is_some_and(|pending| pending.key == request.key)
        {
            return;
        }
        if self.browser.preview_in_flight.is_some() {
            self.browser.preview_pending = Some(request);
            self.cancel_superseded_browser_audio_preview();
        } else {
            self.start_browser_audio_preview(request, cx);
        }
    }

    /// A newer waveform request replaced the pending one; stop the in-flight
    /// job unless the selected row or an active drag still needs its result.
    pub(super) fn cancel_superseded_browser_audio_preview(&self) {
        let Some(in_flight) = self.browser.preview_in_flight.as_ref() else {
            return;
        };
        let needed = self.browser.preview_selected.as_ref() == Some(&in_flight.key)
            || self.browser.active_audio_drag.as_ref().is_some_and(|drag| {
                drag.path == in_flight.key.path && drag.sample_rate == in_flight.key.sample_rate
            });
        if !needed {
            in_flight.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn start_browser_audio_preview(
        &mut self,
        request: PreviewRequest,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        {
            self.browser.preview_preparations += 1;
        }
        self.browser.preview_in_flight = Some(request.clone());
        let sources = self.browser.preview_sources.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path = request.key.path.clone();
            let sample_rate = request.key.sample_rate;
            let cancel = Arc::clone(&request.cancel);
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    sources.prepare_waveform(path, sample_rate, &cancel)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_audio_preview_finished(request, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_audio_preview_finished(
        &mut self,
        request: PreviewRequest,
        result: Result<AudioPreview, PreviewFailure>,
        cx: &mut Context<Self>,
    ) {
        if self
            .browser
            .preview_in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.key == request.key)
        {
            self.browser.preview_in_flight = None;
        }
        match result {
            Ok(preview) => *request.slot.metadata.borrow_mut() = Some(Rc::new(preview)),
            // A superseded job leaves its slot empty, so a later request retries.
            Err(PreviewFailure::Cancelled) => {}
            Err(PreviewFailure::Failed(_) | PreviewFailure::SourceChanged) => {
                *request.slot.error.borrow_mut() =
                    Some("audio preview could not be prepared".into());
            }
        }
        if let Some(active) = self
            .browser
            .active_audio_drag
            .clone()
            .filter(|drag| drag.path == request.key.path)
        {
            self.refresh_browser_audio_drag(&active);
        }
        if let Some(pending) = self.browser.preview_pending.take() {
            self.start_browser_audio_preview(pending, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_key_distinguishes_sample_rate_and_capacity_is_bounded() {
        let a = PreviewKey {
            path: PathBuf::from("a.wav"),
            sample_rate: 44_100,
        };
        let b = PreviewKey {
            path: PathBuf::from("a.wav"),
            sample_rate: 48_000,
        };
        assert_ne!(a, b);
        assert_eq!(PREVIEW_CACHE_CAPACITY, 16);
    }
}
