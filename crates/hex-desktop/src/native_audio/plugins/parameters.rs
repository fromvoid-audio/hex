//! Read-only inventory of the exact retained VST3 controller. One page can
//! be in flight, and a retired binding never supplies rows for its successor.
use super::*;
use hex_audio::plugin_worker::{
    NativeWorkerControllerProgress, NativeWorkerControllerQuery, ParameterPageQuery,
};
use hex_session::service::{PluginParameterQuery, PluginParameterResult, PluginParameterRow};
use std::time::{Duration, Instant};

pub(super) struct ObservedInventory {
    pub(super) query: PluginParameterQuery,
    pub(super) binding: Binding,
    pub(super) rows: Vec<PluginParameterRow>,
}

pub(super) struct Inventory {
    query: PluginParameterQuery,
    binding: Binding,
    class: Option<[u8; 16]>,
    token: Option<NativeWorkerControllerQuery>,
    revision: u64,
    total: Option<u16>,
    rows: Vec<PluginParameterRow>,
    deadline: Instant,
    terminal: Option<Result<(), String>>,
}

impl Inventory {
    pub(super) fn output_closed(&mut self) {
        self.terminal = Some(Err("vst3 parameter output changed".into()));
    }

    // Metadata is observed inside the existing asynchronous query deadline.
    // A busy Root neither blocks request intake nor creates unsupported rows.
    fn observe_class(&mut self, result: Result<[u8; 16], String>, now: Instant) -> bool {
        match result {
            Ok(class) => {
                self.class = Some(class);
                true
            }
            Err(reason)
                if matches!(
                    reason.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) && now < self.deadline =>
            {
                false
            }
            Err(reason) => {
                self.terminal = Some(Err(reason));
                false
            }
        }
    }
}

impl Plugins {
    pub(in crate::native_audio) fn query_parameters(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        query: PluginParameterQuery,
    ) -> Result<(), String> {
        // The editor can see the exact callback ACK before the worker's next
        // poll. Release the submitted fence before accepting a renewed query.
        if self
            .restore
            .as_ref()
            .is_some_and(super::restore::Restore::submitted)
        {
            self.poll_restore(control, Some(query.context));
        }
        if self.parameters.is_some()
            || self.editor.is_some()
            || self.restore.is_some()
            || self.parameter_gesture.is_some()
        {
            return Err("vst3 controller is busy · try again".into());
        }
        let binding = self
            .sources
            .iter()
            .rev()
            .find(|source| {
                source.accepted
                    && !source.retiring
                    && source.device == query.device.value()
                    && source.project_generation == query.context.project_generation
            })
            .ok_or("vst3 parameters need an active hosted instance")?
            .binding;
        self.parameters = Some(Inventory {
            query,
            binding,
            class: None,
            token: None,
            revision: 0,
            total: None,
            rows: Vec::new(),
            deadline: Instant::now() + Duration::from_millis(250),
            terminal: None,
        });
        Ok(())
    }

    pub(in crate::native_audio) fn parameters_ready(&self) -> bool {
        self.parameters
            .as_ref()
            .is_some_and(|inventory| inventory.terminal.is_some())
    }

    pub(in crate::native_audio) fn take_parameters(&mut self) -> Option<PluginParameterResult> {
        if !self.parameters_ready() {
            return None;
        }
        let inventory = self.parameters.take()?;
        let rows = inventory.terminal?.map(|()| inventory.rows);
        self.observed_parameters = rows.as_ref().ok().map(|rows| ObservedInventory {
            query: inventory.query,
            binding: inventory.binding,
            rows: rows.clone(),
        });
        Some(PluginParameterResult {
            query: inventory.query,
            rows,
        })
    }

    pub(super) fn poll_parameters(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        let Some(inventory) = self
            .parameters
            .as_mut()
            .filter(|item| item.terminal.is_none())
        else {
            return;
        };
        let current = context.is_some_and(|context| {
            context.project_generation == inventory.query.context.project_generation
        }) && control.output_identity() == Some(inventory.query.output_identity)
            && self.sources.iter().any(|source| {
                source.binding == inventory.binding && source.accepted && !source.retiring
            });
        if inventory.class.is_none() && current {
            let class = control
                .native_source_metadata(inventory.binding)
                .map(|metadata| metadata.selection().cid);
            if !inventory.observe_class(class, Instant::now()) {
                return;
            }
        }
        let result = if let Some(token) = inventory.token.as_ref() {
            control.poll_native_source_controller_query_on_control(inventory.binding, token)
        } else if !current {
            Err("vst3 parameter target changed")
        } else {
            let Ok(first) = u16::try_from(inventory.rows.len()) else {
                inventory.terminal = Some(Err("vst3 parameter inventory exceeds its limit".into()));
                return;
            };
            let query = ParameterPageQuery {
                first,
                revision: inventory.revision,
                conversion: None,
            };
            match control.begin_native_source_controller_query_on_control(inventory.binding, query)
            {
                Ok(token) => {
                    inventory.token = Some(token);
                    return;
                }
                Err(
                    "native preparation is busy"
                    | "native controller mailbox is busy"
                    | "native controller query is already pending",
                ) if Instant::now() < inventory.deadline => return,
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy")
                if inventory.token.is_some() => {}
            Ok(NativeWorkerControllerProgress::Page(page)) if current => {
                inventory.token = None;
                let total = usize::from(page.total);
                if usize::from(page.first) != inventory.rows.len()
                    || inventory.rows.len() + page.as_slice().len() > total
                    || (inventory.revision != 0 && inventory.revision != page.revision)
                    || inventory
                        .total
                        .is_some_and(|expected| expected != page.total)
                    || (page.as_slice().is_empty() && inventory.rows.len() < total)
                {
                    inventory.terminal = Some(Err("vst3 parameter page differs".into()));
                    return;
                }
                inventory.revision = page.revision;
                inventory.total = Some(page.total);
                inventory
                    .rows
                    .extend(page.as_slice().iter().map(|parameter| {
                        let title = parameter.title.as_utf16();
                        let name = if title.is_empty() {
                            let short = parameter.short_title.as_utf16();
                            if short.is_empty() {
                                format!("parameter {}", parameter.id)
                            } else {
                                String::from_utf16_lossy(short)
                            }
                        } else {
                            String::from_utf16_lossy(title)
                        };
                        let value = parameter
                            .display
                            .map(|display| String::from_utf16_lossy(display.as_utf16()))
                            .filter(|display| !display.is_empty())
                            .unwrap_or_else(|| format!("{:.1}%", parameter.normalized * 100.0));
                        PluginParameterRow {
                            id: parameter.id,
                            name,
                            value,
                            normalized: parameter.normalized,
                            default_normalized: parameter.default_normalized,
                            steps: parameter.step_count,
                            editable: parameter.is_editable(),
                            automation_class: inventory
                                .class
                                .filter(|_| parameter.supports_authored_automation()),
                            page_first: page.first,
                            metadata_revision: page.revision,
                        }
                    }));
                if inventory.rows.len() == total {
                    inventory.terminal = Some(Ok(()));
                } else {
                    inventory.deadline = Instant::now() + Duration::from_millis(250);
                }
            }
            Ok(NativeWorkerControllerProgress::Changed { .. }) => {
                inventory.terminal =
                    Some(Err("vst3 parameters changed · reopen to refresh".into()));
            }
            Err(error) => inventory.terminal = Some(Err(error.into())),
            _ => inventory.terminal = Some(Err("vst3 parameter source unavailable".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(now: Instant) -> Inventory {
        use hex_project::{Command, Created, Editor, Project};
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
        Inventory {
            query: PluginParameterQuery {
                context: AudioSourceContext {
                    project_generation: 1,
                    editor_revision: 1,
                },
                device,
                output_identity: 1,
                request_id: 1,
            },
            binding: Binding {
                root: 1,
                instance: 1,
                graph: 1,
                epoch: 1,
            },
            class: None,
            token: None,
            revision: 0,
            total: None,
            rows: Vec::new(),
            deadline: now + Duration::from_millis(250),
            terminal: None,
        }
    }

    #[test]
    fn busy_metadata_retains_query_until_exact_class_is_available() {
        let now = Instant::now();
        let mut query = inventory(now);
        for reason in ["native preparation is busy", "native source is busy"] {
            assert!(!query.observe_class(Err(reason.into()), now));
            assert!(query.terminal.is_none());
            assert!(query.class.is_none());
            assert!(query.rows.is_empty());
        }
        assert!(query.observe_class(Ok([7; 16]), now));
        assert_eq!(query.class, Some([7; 16]));
        assert!(query.terminal.is_none());
    }

    #[test]
    fn metadata_timeout_and_definite_source_loss_finish_without_partial_rows() {
        let now = Instant::now();
        for (reason, at) in [
            ("native source is busy", now + Duration::from_millis(250)),
            ("native source differs", now),
        ] {
            let mut query = inventory(now);
            assert!(!query.observe_class(Err(reason.into()), at));
            assert_eq!(query.terminal, Some(Err(reason.into())));
            assert!(query.class.is_none());
            assert!(query.rows.is_empty());
        }
    }
}
