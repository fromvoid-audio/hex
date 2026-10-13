use super::*;

/// Shared exclusively among control/lifecycle owners. Callback rendering never
/// locks or clones this root; only its confirmed disposal path transfers a batch.
#[derive(Clone)]
pub(crate) struct NativeStreamRoot {
    inner: Arc<std::sync::Mutex<StreamBridgeControl>>,
    device_epoch: u64,
}
impl NativeStreamRoot {
    pub(crate) fn catalog_lease(&self, id: hex_stream::DomainId) -> Option<CatalogLease> {
        let root = self.inner.try_lock().ok()?;
        if root
            .return_requests
            .iter()
            .any(|flag| flag.load(Acquire) == id.value())
        {
            return None;
        }
        root.reaper.catalog_lease(id)
    }
    pub(crate) fn input_bridge_storage_bytes(&self) -> Option<usize> {
        self.inner.try_lock().ok()?.input_bridge_storage_bytes()
    }
    pub(crate) fn pool_reports(
        &self,
    ) -> Option<[Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED]> {
        Some(self.inner.try_lock().ok()?.pool_reports())
    }
    pub(crate) fn attach(
        control: &mut Control,
        callback: &mut Callback,
        mut bridge: StreamBridgeControl,
    ) -> Self {
        bridge.native_retired = control.retired.take();
        let device_epoch = bridge.device_epoch;
        let root = Self {
            inner: Arc::new(std::sync::Mutex::new(bridge)),
            device_epoch,
        };
        callback.native_stream_root = Some(root.clone());
        root
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, StreamBridgeControl> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn publish(
        &self,
        control: &mut Control,
        request: StreamPublicationRequest,
    ) -> Result<u64, StreamPublishError> {
        match self.inner.try_lock() {
            Ok(mut root) => control.publish_streamed(&mut root, request),
            Err(_) => Err(StreamPublishError {
                reason: "stream control cleanup is busy",
                retryable: true,
                request: Box::new(request),
            }),
        }
    }
    pub(crate) fn device_epoch(&self) -> u64 {
        self.device_epoch
    }
    pub(crate) fn phase(&self) -> Option<hex_stream::MediaCleanupPhase> {
        self.inner.try_lock().ok().map(|root| root.cleanup_phase())
    }
    pub(crate) fn tick(&self, final_retirement: bool) -> hex_stream::MediaCleanupPhase {
        let mut root = self.lock();
        if let Some(mut retired) = root.native_retired.take() {
            let _result = root.reclaim_receiver(&mut retired, false);
            root.native_retired = Some(retired);
        }
        if final_retirement && root.native_callback_ended {
            for index in 0..RETIRED {
                if let Some((id, _)) = root.native_reports[index] {
                    if root.reaper.take_reclaimed_report(id).is_some() {
                        root.return_requests[index].store(0, Release);
                    }
                }
            }
        }
        root.cleanup_phase()
    }
    /// Taking the actual report is the explicit control consumption that frees
    /// the slot. Unread reports keep native domain admission backpressured.
    pub(crate) fn take_report(
        &self,
    ) -> Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)> {
        let mut root = self.inner.try_lock().ok()?;
        for index in 0..RETIRED {
            if let Some((id, report)) = root.native_reports[index] {
                if root.reaper.take_reclaimed_report(id).is_some() {
                    root.native_reports[index] = None;
                    root.domain_contexts[index] = None;
                    root.return_requests[index].store(0, Release);
                    return Some((id, report));
                }
            }
        }
        None
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        let Some(root) = self.native_stream_root.take() else {
            return;
        };
        // Native StreamOwner destroys this closure only after successful pause,
        // on lifecycle/control. Never call this disposal path from audio render.
        let Some(stream) = self.streaming.as_mut() else {
            return;
        };
        for publication in [
            &mut self.active,
            &mut self.deferred,
            &mut self.held_retirement,
        ]
        .into_iter()
        .flatten()
        {
            let _installed = stream.install_publication(publication);
        }
        while let Ok(mut publication) = self.publications.pop() {
            let _installed = stream.install_publication(&mut publication);
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                let mut root = root.lock();
                if let Err(admission) = root.reaper.recover_uninstalled(admission) {
                    let Some(slot) = root
                        .native_failed_admissions
                        .iter_mut()
                        .find(|slot| slot.is_none())
                    else {
                        unreachable!("two domain admissions");
                    };
                    *slot = Some(admission);
                }
            }
            drop(publication); // Confirmed control-side closure disposal.
        }
        let mut root = root.lock();
        for publication in [
            &mut self.active,
            &mut self.deferred,
            &mut self.held_retirement,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                if let Err(admission) = root.reaper.recover_uninstalled(admission) {
                    let Some(slot) = root
                        .native_failed_admissions
                        .iter_mut()
                        .find(|slot| slot.is_none())
                    else {
                        unreachable!("two domain admissions");
                    };
                    *slot = Some(admission);
                }
            }
        }
        if let Some(batch) = stream.batch.take() {
            match root.terminal.try_return_stopped_batch(batch) {
                Ok(()) => root.native_callback_ended = true,
                Err(error) => {
                    root.native_failed_batch = Some(error.into_batch());
                }
            }
        }
    }
}
