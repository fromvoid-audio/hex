//! Fresh-output restore borrows one canonical snapshot. Source, child, worker
//! and endpoint custody remain in that output's existing Root. At most one
//! source advances per poll; no old-output binding or default instance is reused.
use super::*;

pub(super) struct Reopen {
    pub(super) context: AudioSourceContext,
    output: Option<u64>,
    next: usize,
    current: Option<Binding>,
    pub(super) ready: bool,
    cleanup: bool,
}

impl Source {
    fn is_reopening_for(&self, generation: u64) -> bool {
        self.project_generation == generation && self.operation.is_reopen()
    }
}

impl Plugins {
    pub(in crate::native_audio) fn request_reopen(
        &mut self,
        snapshot: &CanonicalAudioSourceSnapshot,
    ) {
        let input = snapshot.effect_preparation();
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(observed) = &self.progress
            && let Ok(mut observed) = observed.try_lock()
        {
            observed.restore_refused = false;
        }
        let mut has_effects = false;
        let visited = input.visit_plugin_effects(|_, _, _| {
            has_effects = true;
            Ok(())
        });
        self.reopen = (snapshot.audio_mode() == DocumentAudioMode::Resident
            && visited.is_ok()
            && has_effects
            && self.host.is_some()
            && self.catalog.is_some())
        .then_some(Reopen {
            context: input.context(),
            output: None,
            next: 0,
            current: None,
            ready: false,
            cleanup: false,
        });
    }
    pub(in crate::native_audio) fn reopen_ready(&self) -> Option<AudioSourceContext> {
        self.reopen
            .as_ref()
            .filter(|restore| restore.ready)
            .map(|restore| restore.context)
    }
    pub(in crate::native_audio) fn hold_reopen(&self, context: AudioSourceContext) -> bool {
        self.reopen
            .as_ref()
            .is_some_and(|restore| restore.context.project_generation == context.project_generation)
    }
    pub(in crate::native_audio) fn check_reopen_publication(
        &self,
        context: AudioSourceContext,
        restoring: bool,
    ) -> Result<(), String> {
        if (self.hold_reopen(context) || restoring)
            && (!restoring || self.reopen_ready() != Some(context))
        {
            return Err("vst3 state restoration owns publication".into());
        }
        Ok(())
    }
    pub(in crate::native_audio) fn finish_reopen(
        &mut self,
        context: AudioSourceContext,
        accepted: bool,
    ) -> bool {
        let Some(restore) = self
            .reopen
            .as_mut()
            .filter(|restore| restore.context == context && restore.ready)
        else {
            return false;
        };
        if accepted {
            self.reopen = None;
        } else {
            restore.ready = false;
            restore.cleanup = true;
            for source in &mut self.sources {
                if source.is_reopening_for(context.project_generation) {
                    source.retiring = true;
                }
            }
        }
        true
    }
    pub(in crate::native_audio) fn poll_reopen(
        &mut self,
        output: &mut WorkerPreparedOutput,
        snapshot: Option<&CanonicalAudioSourceSnapshot>,
    ) {
        let Some(mut restore) = self.reopen.take() else {
            return;
        };
        let generation = restore.context.project_generation;
        let Some(snapshot) = snapshot.filter(|snapshot| snapshot.context() == restore.context)
        else {
            for source in &mut self.sources {
                if source.is_reopening_for(generation) {
                    source.retiring = true;
                }
            }
            return;
        };
        if restore.ready {
            self.reopen = Some(restore);
            return;
        }
        let Some(control) = output.publication.control() else {
            self.reopen = Some(restore);
            return;
        };
        let Some(identity) = control.output_identity() else {
            self.reopen = Some(restore);
            return;
        };
        if restore.output.is_some_and(|old| old != identity) {
            restore.cleanup = true;
        }
        restore.output = Some(identity);
        if restore.cleanup {
            for source in &mut self.sources {
                if source.is_reopening_for(generation) {
                    source.retiring = true;
                }
            }
            if self
                .sources
                .iter()
                .any(|source| source.is_reopening_for(generation))
            {
                self.reopen = Some(restore);
            }
            return;
        }
        let input = snapshot.effect_preparation();
        let mut next = None;
        let mut index = 0;
        let visited = input.visit_plugin_effects(|owner, device, state| {
            if index == restore.next {
                next = Some((owner, device, state));
            }
            index += 1;
            Ok(())
        });
        if visited.is_err() {
            restore.cleanup = true;
            self.reopen = Some(restore);
            return;
        }
        let Some((owner, device, state)) = next else {
            // Refused sources must reach their actual return before the
            // admitted unavailable graph can be installed instead.
            if self
                .sources
                .iter()
                .any(|source| source.retiring && source.is_reopening_for(generation))
            {
                self.reopen = Some(restore);
            } else if self
                .sources
                .iter()
                .any(|source| source.accepted && source.is_reopening_for(generation))
            {
                restore.ready = true;
                self.reopen = Some(restore);
            }
            return;
        };
        if let Some(binding) = restore.current {
            let Some(source) = self
                .sources
                .iter_mut()
                .find(|source| source.binding == binding)
            else {
                restore.cleanup = true;
                self.reopen = Some(restore);
                return;
            };
            #[cfg(not(all(test, feature = "allocation-probe")))]
            let presentation = None;
            #[cfg(all(test, feature = "allocation-probe"))]
            let presentation = self.presentation;
            let result = self
                .host
                .as_ref()
                .ok_or_else(|| "qualified vst3 helper unavailable".to_owned())
                .and_then(|host| advance_source(host, presentation, control, source));
            match result {
                Ok(Some(restored)) if restored.processing_eq(state) => {
                    source.accepted = true;
                    restore.current = None;
                    restore.next += 1;
                }
                Ok(None) => {}
                Ok(Some(_)) => {
                    source.retiring = true;
                    restore.current = None;
                    restore.next += 1;
                }
                Err(error) => {
                    #[cfg(all(test, feature = "allocation-probe"))]
                    if let Some(observed) = &self.progress
                        && let Ok(mut observed) = observed.try_lock()
                    {
                        observed.restore_refused = true;
                    }
                    let _ = error;
                    source.retiring = true;
                    restore.current = None;
                    restore.next += 1;
                }
            }
            self.reopen = Some(restore);
            return;
        }
        if !self.configured || self.sources.len() == self.sources.capacity() {
            restore.next += 1;
            self.reopen = Some(restore);
            return;
        }
        // Catalog results are copied choices only. Drop the bounded catalog
        // before Host inspects files and starts its original restore protocol.
        let selected = (|| {
            let identity = Hosting::saved_identity_on_control(state)?;
            let catalog = hex_plugin_catalog::Catalog::load(
                self.catalog.as_deref().ok_or("vst3 catalog unavailable")?,
            )?;
            catalog.resolve_effect(
                identity.class.bytes(),
                identity.binary.executable_sha256,
                identity.binary.info_sha256,
            )
        })();
        let binding = selected.and_then(|selected| {
            self.host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?
                .begin_on_control(
                    control,
                    EffectSelection {
                        module: &selected.module,
                        executable: &selected.executable_relative_path,
                        cid: selected.cid,
                        executable_sha256: &selected.executable_sha256,
                        info_sha256: &selected.info_sha256,
                    },
                    owner,
                    device,
                    restore.context.editor_revision.max(1),
                    output.sample_rate,
                    Some(state.clone()),
                )
        });
        match binding {
            Ok(binding) => {
                self.sources.push(Source {
                    binding,
                    owner,
                    device,
                    project_generation: restore.context.project_generation,
                    operation: SourceOperation::Reopen,
                    accepted: false,
                    retiring: false,
                    queued: false,
                    observes_latency: false,
                    editor_open: false,
                    #[cfg(all(test, feature = "allocation-probe"))]
                    observed_latency_receipt: false,
                    #[cfg(all(test, feature = "allocation-probe"))]
                    attempts: 0,
                });
                restore.current = Some(binding);
            }
            // Work contention allocated no slot or native owner. Retry this
            // same saved device; it is not a missing-source terminal.
            Err(reason) if reason == "native preparation is busy" => {}
            Err(_) => restore.next += 1,
        }
        self.reopen = Some(restore);
    }
}
