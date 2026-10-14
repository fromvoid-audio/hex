use super::*;

/// Shared exclusively among control/lifecycle owners. Callback rendering never
/// locks or clones this root; only its confirmed disposal path transfers a batch.
#[derive(Clone)]
pub(crate) struct NativeStreamRoot {
    inner: Arc<std::sync::Mutex<NativeStreamState>>,
    device_epoch: u64,
}
struct NativeStreamState {
    bridge: StreamBridgeControl,
    preparation: Option<crate::stream_preparation::PreparationSlot>,
    closing: bool,
}
impl NativeStreamRoot {
    fn preparation_storage_bytes() -> Option<usize> {
        std::mem::size_of::<std::sync::Mutex<NativeStreamState>>()
            .checked_sub(std::mem::size_of::<std::sync::Mutex<StreamBridgeControl>>())
    }
    pub(crate) fn current_preparation_snapshot(
        &self,
    ) -> Option<crate::stream_preparation::PreparationSnapshot> {
        self.inner
            .try_lock()
            .ok()?
            .preparation
            .as_ref()?
            .current_snapshot()
    }
    #[cfg(test)]
    pub(crate) fn preparation_original_addresses(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> Option<(usize, usize, usize, usize)> {
        self.lock().preparation.as_ref()?.original_addresses(token)
    }
    pub(crate) fn begin_preparation(
        &self,
        input: crate::stream_preparation::CandidateInput,
        rate: u32,
    ) -> Result<
        crate::stream_preparation::PreparationToken,
        crate::stream_preparation::PreparationRefusal,
    > {
        let Ok(mut root) = self.inner.try_lock() else {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream control cleanup is busy",
                input: Box::new(input),
            });
        };
        if root.closing {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream preparation is closing",
                input: Box::new(input),
            });
        }
        let Some(slot) = root.preparation.as_mut() else {
            return Err(crate::stream_preparation::PreparationRefusal {
                reason: "stream preparation lifecycle is busy",
                input: Box::new(input),
            });
        };
        slot.begin(
            input,
            rate,
            self.device_epoch,
            Self::preparation_storage_bytes().unwrap_or(usize::MAX),
        )
    }
    pub(crate) fn preparation_snapshot(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> Option<crate::stream_preparation::PreparationSnapshot> {
        self.inner
            .try_lock()
            .ok()?
            .preparation
            .as_ref()?
            .snapshot(token)
    }
    pub(crate) fn cancel_preparation(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> bool {
        self.inner
            .try_lock()
            .ok()
            .and_then(|mut root| root.preparation.as_mut().map(|slot| slot.cancel(token)))
            .unwrap_or(false)
    }
    pub(crate) fn consume_preparation(
        &self,
        token: crate::stream_preparation::PreparationToken,
    ) -> bool {
        self.inner
            .try_lock()
            .ok()
            .and_then(|mut root| root.preparation.as_mut().map(|slot| slot.consume(token)))
            .unwrap_or(false)
    }
    pub(crate) fn publish_preparation(
        &self,
        control: &mut Control,
        token: crate::stream_preparation::PreparationToken,
        current: crate::stream_preparation::CandidateIdentity,
        timeline_epoch: u64,
        policy: PublicationPolicy,
    ) -> Result<Option<u64>, &'static str> {
        let mut root = self
            .inner
            .try_lock()
            .map_err(|_| "stream control cleanup is busy")?;
        if root.closing {
            return Err("stream preparation is closing");
        }
        let mut occupied = [None; super::super::RETIRED];
        root.bridge
            .reaper
            .visit_domains(|index, id| occupied[index] = Some(id));
        let NativeStreamState {
            bridge,
            preparation,
            ..
        } = &mut *root;
        let Some(slot) = preparation.as_mut() else {
            return Err("stream preparation lifecycle is busy");
        };
        slot.publish(
            token,
            current,
            timeline_epoch,
            policy,
            &occupied,
            |request| control.publish_streamed(bridge, request),
        )
    }
    pub(crate) fn cancel_all_preparation(&self) {
        let mut root = self.lock();
        root.closing = true;
        if let Some(slot) = root.preparation.as_mut() {
            slot.close();
        }
    }
    pub(crate) fn catalog_lease(&self, id: hex_stream::DomainId) -> Option<CatalogLease> {
        let root = self.inner.try_lock().ok()?;
        if root
            .bridge
            .return_requests
            .iter()
            .any(|flag| flag.load(Acquire) == id.value())
        {
            return None;
        }
        root.bridge.reaper.catalog_lease(id)
    }
    pub(crate) fn input_bridge_storage_bytes(&self) -> Option<usize> {
        self.inner
            .try_lock()
            .ok()?
            .bridge
            .input_bridge_storage_bytes()?
            .checked_add(
                std::mem::size_of::<std::sync::Mutex<NativeStreamState>>()
                    .checked_sub(std::mem::size_of::<std::sync::Mutex<StreamBridgeControl>>())?,
            )
    }
    pub(crate) fn pool_reports(
        &self,
    ) -> Option<[Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)>; RETIRED]> {
        Some(self.inner.try_lock().ok()?.bridge.pool_reports())
    }
    pub(crate) fn attach(
        control: &mut Control,
        callback: &mut Callback,
        mut bridge: StreamBridgeControl,
    ) -> Self {
        bridge.native_retired = control.retired.take();
        let device_epoch = bridge.device_epoch;
        let root = Self {
            inner: Arc::new(std::sync::Mutex::new(NativeStreamState {
                bridge,
                preparation: Some(crate::stream_preparation::PreparationSlot::default()),
                closing: false,
            })),
            device_epoch,
        };
        callback.native_stream_root = Some(root.clone());
        root
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, NativeStreamState> {
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
            Ok(mut root) => control.publish_streamed(&mut root.bridge, request),
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
        self.inner.try_lock().ok().map(|root| {
            match (
                root.preparation.as_ref().map(|slot| slot.phase()),
                root.bridge.cleanup_phase(),
            ) {
                (Some(hex_stream::MediaCleanupPhase::FailedIncomplete), _) => {
                    hex_stream::MediaCleanupPhase::FailedIncomplete
                }
                (Some(hex_stream::MediaCleanupPhase::Complete), phase) => phase,
                _ => hex_stream::MediaCleanupPhase::Pending,
            }
        })
    }
    pub(crate) fn tick(&self, final_retirement: bool) -> hex_stream::MediaCleanupPhase {
        // Request join/graph/Status destruction must occur OUTSIDE root mutex.
        // None is a busy marker while lifecycle exclusively owns the fixed slot.
        let (slot, closing) = {
            let mut root = self.lock();
            root.closing |= final_retirement;
            (root.preparation.take(), root.closing)
        };
        let preparation_phase = if let Some(mut slot) = slot {
            slot.tick(closing);
            let phase = slot.phase();
            self.lock().preparation = Some(slot);
            phase
        } else {
            hex_stream::MediaCleanupPhase::Pending
        };
        let mut root = self.lock();
        if let Some(mut retired) = root.bridge.native_retired.take() {
            let _result = root.bridge.reclaim_receiver(&mut retired, false);
            root.bridge.native_retired = Some(retired);
        }
        if final_retirement && root.bridge.native_callback_ended {
            for index in 0..RETIRED {
                if let Some((id, _)) = root.bridge.native_reports[index] {
                    if root.bridge.reaper.take_reclaimed_report(id).is_some() {
                        root.bridge.return_requests[index].store(0, Release);
                    }
                }
            }
        }
        match (preparation_phase, root.bridge.cleanup_phase()) {
            (hex_stream::MediaCleanupPhase::FailedIncomplete, _)
            | (_, hex_stream::MediaCleanupPhase::FailedIncomplete) => {
                hex_stream::MediaCleanupPhase::FailedIncomplete
            }
            (hex_stream::MediaCleanupPhase::Complete, hex_stream::MediaCleanupPhase::Complete) => {
                hex_stream::MediaCleanupPhase::Complete
            }
            _ => hex_stream::MediaCleanupPhase::Pending,
        }
    }
    /// Taking the actual report is the explicit control consumption that frees
    /// the slot. Unread reports keep native domain admission backpressured.
    pub(crate) fn take_report(
        &self,
    ) -> Option<(hex_stream::DomainId, hex_stream::ReportedShutdown)> {
        let mut root = self.inner.try_lock().ok()?;
        for index in 0..RETIRED {
            if let Some((id, report)) = root.bridge.native_reports[index] {
                if root.bridge.reaper.take_reclaimed_report(id).is_some() {
                    root.bridge.native_reports[index] = None;
                    root.bridge.domain_contexts[index] = None;
                    root.bridge.return_requests[index].store(0, Release);
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
        for publication in [&mut self.active, &mut self.deferred].into_iter().flatten() {
            let _installed = stream.install_publication(publication);
        }
        if let Some(binding) = self
            .held_retirement
            .as_mut()
            .and_then(GraphReturn::stream_source_mut)
        {
            let _installed = stream.install_binding(binding);
        }
        while let Ok(mut publication) = self.publications.pop() {
            let _installed = stream.install_publication(&mut publication);
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                let mut root = root.lock();
                if let Err(admission) = root.bridge.reaper.recover_uninstalled(admission) {
                    let Some(slot) = root
                        .bridge
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
        for publication in [&mut self.active, &mut self.deferred].into_iter().flatten() {
            if let Some(admission) = publication
                .stream_source
                .as_mut()
                .and_then(|binding| binding.admission.take())
            {
                if let Err(admission) = root.bridge.reaper.recover_uninstalled(admission) {
                    let Some(slot) = root
                        .bridge
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
        if let Some(admission) = self
            .held_retirement
            .as_mut()
            .and_then(GraphReturn::stream_source_mut)
            .and_then(|binding| binding.admission.take())
        {
            if let Err(admission) = root.bridge.reaper.recover_uninstalled(admission) {
                let slot = root
                    .bridge
                    .native_failed_admissions
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .expect("at most two actual domain admissions");
                *slot = Some(admission);
            }
        }
        if let Some(batch) = stream.batch.take() {
            match root.bridge.terminal.try_return_stopped_batch(batch) {
                Ok(()) => root.bridge.native_callback_ended = true,
                Err(error) => {
                    root.bridge.native_failed_batch = Some(error.into_batch());
                }
            }
        }
    }
}

#[cfg(test)]
mod preparation_race_tests {
    use super::*;
    #[test]
    fn retirement_close_survives_slot_temporarily_owned_by_lifecycle() {
        let (mut control, mut callback, bridge) = channel_with_streaming(48_000, 1, 3).unwrap();
        let root = NativeStreamRoot::attach(&mut control, &mut callback, bridge);
        let (input, _file) = super::super::tests::facade_input(108);
        let token = root.begin_preparation(input, 48_000).unwrap();
        let mut slot = root.lock().preparation.take().unwrap();
        root.cancel_all_preparation(); // Close flag must persist when slot is out.
        let (input, _second_file) = super::super::tests::facade_input(109);
        assert_eq!(
            root.begin_preparation(input, 48_000).unwrap_err().reason,
            "stream preparation is closing"
        );
        let closing = root.lock().closing;
        assert!(closing);
        slot.tick(closing);
        assert!(slot.snapshot(token).is_none());
        root.lock().preparation = Some(slot);
        drop(control);
        drop(callback);
        root.tick(true);
        assert_eq!(root.phase(), Some(hex_stream::MediaCleanupPhase::Complete));
    }
}
