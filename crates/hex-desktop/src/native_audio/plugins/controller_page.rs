//! One source-bound controller observation on the existing native worker.
//! Copied metadata is never an edit, graph acknowledgement or source owner.
use super::*;
use hex_audio::plugin_worker::{
    NativeWorkerControllerProgress, NativeWorkerControllerQuery, ParameterPageQuery,
    ParameterPlainValue, ParameterText,
};
use hex_session::service::{
    PluginControllerPage, PluginControllerPageRequest, PluginControllerPageResult,
    PluginControllerParameter, PluginControllerPlainValue,
};

pub(super) struct PendingControllerPage {
    request: PluginControllerPageRequest,
    binding: Binding,
    output: u64,
    expected: NativeDeviceState,
    // None means no SDK operation has been admitted yet; the same bounded
    // request remains pending across transient control-owner contention.
    token: Option<NativeWorkerControllerQuery>,
    observation: Option<NativeWorkerControllerProgress>,
    detached: bool,
    result: Option<Result<PluginControllerPageResult, String>>,
}

impl Plugins {
    pub(super) fn controller_page_identity(
        &self,
        request: PluginControllerPageRequest,
    ) -> Option<(Binding, u64)> {
        self.controller_page
            .as_ref()
            .filter(|pending| pending.request == request)
            .map(|pending| (pending.binding, pending.output))
    }
    pub(super) fn controller_page_ready(&self, request: PluginControllerPageRequest) -> bool {
        self.controller_page
            .as_ref()
            .is_some_and(|pending| pending.request == request && pending.result.is_some())
    }
    pub(in crate::native_audio) fn begin_controller_page(
        &mut self,
        output: &mut WorkerPreparedOutput,
        current: Option<AudioSourceContext>,
        request: PluginControllerPageRequest,
        expected: &NativeDeviceState,
    ) -> Result<(), String> {
        if self.controller_page.is_some() {
            return Err("vst3 controller query is already pending".into());
        }
        if current != Some(request.context) || request.device_id == 0 {
            return Err("vst3 controller query context changed".into());
        }
        let source = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.project_generation == request.context.project_generation
                    && source.device == request.device_id
            })
            .ok_or("vst3 controller source is unavailable")?;
        let control = output
            .publication
            .control()
            .ok_or("vst3 output is unavailable")?;
        let output_id = control
            .output_identity()
            .ok_or("vst3 output identity is unavailable")?;
        self.controller_page = Some(PendingControllerPage {
            request,
            binding: source.binding,
            output: output_id,
            expected: expected.clone(),
            token: None,
            observation: None,
            detached: false,
            result: None,
        });
        Ok(())
    }

    pub(in crate::native_audio) fn take_controller_page(
        &mut self,
        request: PluginControllerPageRequest,
        current: Option<AudioSourceContext>,
    ) -> Result<PluginControllerPageResult, String> {
        let pending = self
            .controller_page
            .as_mut()
            .ok_or("vst3 controller query is unavailable")?;
        if pending.request != request {
            return Err("vst3 controller query identity differs".into());
        }
        let Some(result) = pending.result.take() else {
            return Ok(PluginControllerPageResult::Pending);
        };
        self.controller_page = None;
        if current != Some(request.context) {
            return Err("vst3 controller query context changed".into());
        }
        result
    }

    pub(super) fn detach_controller_page(&mut self) {
        if let Some(pending) = self.controller_page.as_mut() {
            pending.detached = true;
            if pending.observation.is_some() || pending.result.is_some() {
                pending.result = Some(Err("vst3 controller output changed".into()));
            }
        }
    }

    pub(in crate::native_audio) fn poll_detached_controller_page(&mut self) {
        if let Some(pending) = self.controller_page.as_mut()
            && pending.detached
            && pending.result.is_none()
            && pending
                .token
                .as_ref()
                .is_none_or(|token| token.original_worker_released_on_control())
        {
            // Worker release only retires this query; it does not assert that
            // the native source itself was destroyed rather than quiesced.
            pending.result = Some(Err("vst3 controller output changed".into()));
        }
    }

    pub(super) fn poll_controller_page(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        current: Option<AudioSourceContext>,
    ) {
        let Some(pending) = self.controller_page.as_mut() else {
            return;
        };
        if pending.result.is_some() {
            return;
        }
        if control.output_identity() != Some(pending.output) {
            pending.detached = true;
        }
        if pending.detached {
            self.poll_detached_controller_page();
            return;
        }
        if pending.token.is_none() {
            if current != Some(pending.request.context) {
                pending.result = Some(Err("vst3 controller query context changed".into()));
                return;
            }
            match control
                .native_source_matches_accepted_state_on_control(pending.binding, &pending.expected)
            {
                Ok(true) => {}
                Ok(false) => {
                    pending.result = Some(Err("vst3 controller source changed".into()));
                    return;
                }
                Err("native preparation is busy") => return,
                Err(error) => {
                    pending.result = Some(Err(error.into()));
                    return;
                }
            }
            match control.begin_native_source_controller_query_on_control(
                pending.binding,
                ParameterPageQuery {
                    first: pending.request.first,
                    revision: pending.request.revision,
                    conversion: None,
                },
            ) {
                Ok(token) => pending.token = Some(token),
                Err("native preparation is busy" | "native controller mailbox is busy") => return,
                Err("native controller output is closing") => {
                    pending.detached = true;
                    return;
                }
                Err(error) => {
                    pending.result = Some(Err(error.into()));
                    return;
                }
            }
        }
        let Some(token) = pending.token.as_ref() else {
            return;
        };
        if pending.observation.is_none() {
            match control.poll_native_source_controller_query_on_control(pending.binding, token) {
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                    return;
                }
                Ok(NativeWorkerControllerProgress::Failed) => {
                    match control.drain_stopped_native_source_controller_query_on_control(
                        pending.binding,
                        token,
                    ) {
                        Ok(true) => {
                            pending.result = Some(Err("vst3 controller query stopped".into()))
                        }
                        Ok(false) | Err("native preparation is busy") => {}
                        Err(_) if token.original_worker_released_on_control() => {
                            pending.result = Some(Err("vst3 controller worker changed".into()));
                        }
                        Err(_) => { /* retain the exact original token */ }
                    }
                    return;
                }
                Ok(NativeWorkerControllerProgress::Discarded) => {
                    pending.result = Some(Err("vst3 controller source changed".into()));
                    return;
                }
                Ok(value) => pending.observation = Some(value),
                Err("native controller output is closing") => {
                    pending.detached = true;
                    return;
                }
                Err(_) => {
                    if token.original_worker_released_on_control() {
                        pending.result = Some(Err("vst3 controller worker changed".into()));
                    }
                    return;
                }
            }
        }
        if current != Some(pending.request.context) {
            pending.result = Some(Err("vst3 controller query context changed".into()));
            return;
        }
        match control
            .native_source_matches_accepted_state_on_control(pending.binding, &pending.expected)
        {
            Err("native preparation is busy") => return,
            Err(error) => {
                pending.result = Some(Err(error.into()));
                return;
            }
            Ok(false) => {
                pending.result = Some(Err("vst3 controller source changed".into()));
                return;
            }
            Ok(true) => {}
        }
        let Some(observation) = pending.observation.take() else {
            return;
        };
        pending.result = Some(match observation {
            NativeWorkerControllerProgress::Page(page) => copy_page(pending.request, &page),
            NativeWorkerControllerProgress::Changed { revision, total } => {
                Ok(PluginControllerPageResult::Changed { revision, total })
            }
            _ => Err("vst3 controller query returned an unexpected result".into()),
        });
    }
}

fn text(value: ParameterText) -> Result<String, String> {
    String::from_utf16(value.as_utf16()).map_err(|_| "vst3 controller text is invalid".into())
}

fn plain(value: ParameterPlainValue) -> PluginControllerPlainValue {
    match value {
        ParameterPlainValue::Finite(value) => PluginControllerPlainValue::Finite(value),
        ParameterPlainValue::NegativeInfinity => PluginControllerPlainValue::NegativeInfinity,
        ParameterPlainValue::PositiveInfinity => PluginControllerPlainValue::PositiveInfinity,
        ParameterPlainValue::Unavailable => PluginControllerPlainValue::Unavailable,
    }
}

fn copy_page(
    request: PluginControllerPageRequest,
    page: &hex_audio::plugin_worker::ParameterPage,
) -> Result<PluginControllerPageResult, String> {
    if page.first != request.first || (request.revision != 0 && page.revision != request.revision) {
        return Err("vst3 controller page identity differs".into());
    }
    let mut parameters = Vec::new();
    parameters
        .try_reserve_exact(page.as_slice().len())
        .map_err(|_| "vst3 controller page allocation failed")?;
    for value in page.as_slice() {
        parameters.push(PluginControllerParameter {
            id: value.id,
            flags: value.flags,
            step_count: value.step_count,
            unit_id: value.unit_id,
            editable: value.is_editable(),
            normalized: value.normalized,
            default_normalized: value.default_normalized,
            plain: plain(value.plain),
            default_plain: plain(value.default_plain),
            title: text(value.title)?,
            short_title: text(value.short_title)?,
            units: text(value.units)?,
            display: value.display.map(text).transpose()?,
            default_display: value.default_display.map(text).transpose()?,
        });
    }
    Ok(PluginControllerPageResult::Page(PluginControllerPage {
        request,
        revision: page.revision,
        total: page.total,
        parameters,
    }))
}
