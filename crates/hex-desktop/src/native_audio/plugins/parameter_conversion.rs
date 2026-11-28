//! One copied conversion on the exact retained controller. The worker token
//! stays with this operation until its terminal, including after replacement.
use super::*;
use hex_audio::plugin_worker::{
    NativeWorkerControllerProgress, NativeWorkerControllerQuery, ParameterConversion,
    ParameterConversionStatus, ParameterPageQuery, ParameterText,
};
use hex_audio::prepared::PublicationControl;
use hex_session::service::{
    PluginParameterConversion, PluginParameterConversionQuery, PluginParameterConversionResult,
    PluginParameterConvertedValue,
};
use std::time::{Duration, Instant};

pub(super) struct Conversion {
    query: PluginParameterConversionQuery,
    request: ParameterPageQuery,
    token: Option<NativeWorkerControllerQuery>,
    deadline: Instant,
    result: Option<Result<PluginParameterConvertedValue, String>>,
    failure: Option<String>,
    output_closed: bool,
}

impl Conversion {
    pub(super) fn binding(&self) -> Binding {
        self.query.source
    }
    pub(super) fn has_pending_token(&self) -> bool {
        self.token.is_some()
    }
    pub(super) fn output_closed(&mut self) {
        self.output_closed = true;
        self.failure
            .get_or_insert_with(|| "vst3 conversion output changed".into());
        if self
            .token
            .as_ref()
            .is_none_or(|token| token.original_worker_released_on_control())
        {
            self.result = Some(Err("vst3 conversion output changed".into()));
        }
    }
}

fn current_source(sources: &[Source], query: &PluginParameterConversionQuery) -> bool {
    sources
        .iter()
        .rev()
        .find(|source| {
            source.accepted
                && !source.retiring
                && source.device == query.device.value()
                && source.project_generation == query.context.project_generation
        })
        .is_some_and(|source| source.binding == query.source)
}

impl Plugins {
    fn blocked_by_parameter_gesture(&self, query: &PluginParameterConversionQuery) -> bool {
        self.parameter_gesture.as_ref().is_some_and(|gesture| {
            !matches!(query.operation, PluginParameterConversion::Format(_))
                || !gesture.allows_read_only_format(query.source, query.parameter)
        })
    }

    pub(in crate::native_audio) fn query_parameter_conversion(
        &mut self,
        control: &mut PublicationControl,
        query: PluginParameterConversionQuery,
    ) -> Result<(), String> {
        if self.parameter_conversion.is_some()
            || self.editor_holds(query.source)
            || self.restore.is_some()
            || self.blocked_by_parameter_gesture(&query)
        {
            return Err("vst3 controller is busy · try again".into());
        }
        if control.output_identity() != Some(query.output_identity)
            || !current_source(&self.sources, &query)
        {
            return Err("vst3 parameter target changed".into());
        }
        let conversion = match &query.operation {
            PluginParameterConversion::Format(normalized) => ParameterConversion::Format {
                id: query.parameter,
                normalized: *normalized,
            },
            PluginParameterConversion::Parse(text) => ParameterConversion::Parse {
                id: query.parameter,
                text: ParameterText::from_text(text.as_str()).map_err(|error| error.to_string())?,
            },
        };
        let request = ParameterPageQuery {
            first: 0,
            revision: 0,
            conversion: Some(conversion),
        };
        request.validate().map_err(|error| error.to_string())?;
        // Yield a background enumeration at its next page boundary. Its
        // admitted token still belongs to the existing inventory operation.
        if let Some(inventory) = self.parameters.as_mut() {
            inventory.request_yield();
        }
        self.parameter_conversion = Some(Conversion {
            query,
            request,
            token: None,
            deadline: Instant::now() + Duration::from_millis(250),
            result: None,
            failure: None,
            output_closed: false,
        });
        Ok(())
    }

    /// Drain an in-flight conversion token before a new gesture `Begin` uses
    /// the controller mailbox. The completed result stays queued for Session.
    pub(super) fn drain_parameter_conversion_for_control(
        &mut self,
        control: &mut PublicationControl,
        context: AudioSourceContext,
        cancellation: Option<&hex_session::service::RequestCancellation>,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(150);
        while self
            .parameter_conversion
            .as_ref()
            .is_some_and(|item| item.result.is_none())
        {
            edits::check_cancel(cancellation)?;
            self.poll_parameter_conversion(control, Some(context));
            if self.parameter_conversion_ready() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("vst3 controller is still formatting · try again".into());
            }
            edits::poll_control(control);
        }
        Ok(())
    }

    pub(in crate::native_audio) fn parameter_conversion_ready(&self) -> bool {
        self.parameter_conversion.as_ref().is_some_and(|item| {
            item.result.is_some()
                || (item.output_closed
                    && item
                        .token
                        .as_ref()
                        .is_none_or(|token| token.original_worker_released_on_control()))
        })
    }

    pub(in crate::native_audio) fn take_parameter_conversion(
        &mut self,
        output: Option<u64>,
    ) -> Option<PluginParameterConversionResult> {
        if !self.parameter_conversion_ready() {
            return None;
        }
        let mut item = self.parameter_conversion.take()?;
        if item.output_closed && item.result.is_none() {
            item.output_closed();
        }
        let current = output == Some(item.query.output_identity)
            && current_source(&self.sources, &item.query);
        Some(PluginParameterConversionResult {
            query: item.query,
            value: if current {
                item.result?
            } else {
                Err("vst3 parameter target changed".into())
            },
        })
    }

    pub(super) fn poll_parameter_conversion(
        &mut self,
        control: &mut PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        let Some(item) = self
            .parameter_conversion
            .as_mut()
            .filter(|item| item.result.is_none())
        else {
            return;
        };
        if item.output_closed {
            item.output_closed();
            return;
        }
        let current = context == Some(item.query.context)
            && control.output_identity() == Some(item.query.output_identity)
            && current_source(&self.sources, &item.query);
        let progress = if let Some(token) = &item.token {
            control.poll_native_source_controller_query_on_control(item.query.source, token)
        } else if self.restore.is_some() {
            return;
        } else if !current {
            Err("vst3 parameter target changed")
        } else {
            match control
                .begin_native_source_controller_query_on_control(item.query.source, item.request)
            {
                Ok(token) => {
                    item.token = Some(token);
                    return;
                }
                Err(
                    "native preparation is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) if Instant::now() < item.deadline => return,
                Err(reason) => Err(reason),
            }
        };
        if matches!(&progress, Ok(progress) if !matches!(progress, NativeWorkerControllerProgress::Pending))
        {
            item.token = None;
        }
        match progress {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy")
                if item.token.is_some() => {}
            Ok(NativeWorkerControllerProgress::Conversion(converted)) if current => {
                item.result = Some(if let Some(reason) = item.failure.take() {
                    Err(reason)
                } else {
                    match converted.status {
                        ParameterConversionStatus::Converted => match converted.normalized {
                            Some(normalized) => {
                                let display = converted
                                    .display
                                    .map(|text| String::from_utf16_lossy(text.as_utf16()));
                                if matches!(
                                    item.query.operation,
                                    PluginParameterConversion::Format(_)
                                ) && display.is_none()
                                {
                                    Err("vst3 parameter formatting unsupported".into())
                                } else {
                                    Ok(PluginParameterConvertedValue {
                                        normalized,
                                        display,
                                    })
                                }
                            }
                            None => Err("vst3 conversion returned no value".into()),
                        },
                        ParameterConversionStatus::Refused => {
                            Err("vst3 parameter value refused".into())
                        }
                        ParameterConversionStatus::Unsupported => {
                            Err("vst3 parameter conversion unsupported".into())
                        }
                        ParameterConversionStatus::Missing => {
                            Err("vst3 parameter is missing".into())
                        }
                    }
                });
            }
            Ok(_) if !current => item.result = Some(Err("vst3 parameter target changed".into())),
            Ok(_) => item.result = Some(Err("vst3 parameter conversion unavailable".into())),
            Err(reason)
                if item
                    .token
                    .as_ref()
                    .is_some_and(|token| !token.original_worker_released_on_control()) =>
            {
                item.failure.get_or_insert_with(|| reason.into());
            }
            Err(reason) => {
                item.result = Some(Err(item.failure.take().unwrap_or_else(|| reason.into())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    #[test]
    fn replaced_source_cannot_supply_a_completed_conversion() {
        let mut project = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Device(device) = project
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        };
        let binding = Binding {
            root: 1,
            instance: 1,
            graph: 1,
            epoch: 1,
        };
        let query = PluginParameterConversionQuery {
            context,
            device,
            output_identity: 1,
            request_id: 1,
            source: binding,
            parameter: 7,
            operation: PluginParameterConversion::Format(0.5),
        };
        let source = |binding| Source {
            binding,
            owner: ProcessorChainOwner::Main,
            device: device.value(),
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: false,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            open_editor_group: None,
            provider: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        };
        let mut plugins = Plugins::default();
        plugins.sources.push(source(binding));
        assert!(current_source(&plugins.sources, &query));
        plugins.parameter_conversion = Some(Conversion {
            query: query.clone(),
            request: ParameterPageQuery::default(),
            token: None,
            deadline: Instant::now(),
            result: Some(Ok(PluginParameterConvertedValue {
                normalized: 0.5,
                display: Some("double".into()),
            })),
            failure: None,
            output_closed: false,
        });
        plugins.sources.push(source(Binding {
            epoch: 2,
            ..binding
        }));
        assert!(!current_source(&plugins.sources, &query));
        let result = plugins.take_parameter_conversion(Some(1)).unwrap();
        assert_eq!(result.query, query);
        assert!(result.value.is_err());

        plugins.parameter_conversion = Some(Conversion {
            query,
            request: ParameterPageQuery::default(),
            token: None,
            deadline: Instant::now(),
            result: None,
            failure: None,
            output_closed: false,
        });
        plugins.output_closed();
        assert!(plugins.parameter_conversion_ready());
        assert!(
            plugins
                .take_parameter_conversion(None)
                .unwrap()
                .value
                .is_err()
        );
    }

    #[test]
    fn read_only_format_is_permitted_only_for_active_pre_commit_gesture_target() {
        use hex_session::device_gesture::{PluginParameterOrigin, PluginParameterValue};
        use hex_session::service::PluginParameterText;

        let mut project = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
        let Created::Device(device) = project
            .execute(Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 1,
        };
        let binding = Binding {
            root: 1,
            instance: 1,
            graph: 1,
            epoch: 1,
        };
        let origin = PluginParameterOrigin {
            context,
            device,
            parameter: 7,
            page_first: 0,
            metadata_revision: 1,
            initial: PluginParameterValue::new(0.25).unwrap(),
        };
        let mut plugins = Plugins {
            parameter_gesture: Some(super::parameter_gesture::Gesture::test_active(
                origin, binding, 1, None,
            )),
            ..Plugins::default()
        };
        let query = |source, parameter, operation| PluginParameterConversionQuery {
            context,
            device,
            output_identity: 1,
            request_id: 1,
            source,
            parameter,
            operation,
        };

        assert!(
            !plugins.blocked_by_parameter_gesture(&query(
                binding,
                7,
                PluginParameterConversion::Format(0.5)
            )),
            "read-only Format for the active gesture target must be admitted pre-commit"
        );
        assert!(
            plugins.blocked_by_parameter_gesture(&query(
                binding,
                7,
                PluginParameterConversion::Parse(PluginParameterText::new("0.5").unwrap())
            )),
            "Parse must remain blocked during an active gesture"
        );
        assert!(
            plugins.blocked_by_parameter_gesture(&query(
                binding,
                8,
                PluginParameterConversion::Format(0.5)
            )),
            "Format for a different parameter must remain blocked"
        );
        assert!(
            plugins.blocked_by_parameter_gesture(&query(
                Binding {
                    epoch: 2,
                    ..binding
                },
                7,
                PluginParameterConversion::Format(0.5)
            )),
            "Format for a different source binding must remain blocked"
        );

        plugins.parameter_gesture = Some(super::parameter_gesture::Gesture::test_active(
            origin,
            binding,
            1,
            Some((context, 1)),
        ));
        assert!(
            plugins.blocked_by_parameter_gesture(&query(
                binding,
                7,
                PluginParameterConversion::Format(0.5)
            )),
            "Format must be blocked once gesture commit has started"
        );
    }
}
