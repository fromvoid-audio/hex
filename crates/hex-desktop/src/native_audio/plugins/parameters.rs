//! Read-only inventory of the exact retained VST3 controller. One page can
//! be in flight, and a retired binding never supplies rows for its successor.
//! Each page is a fresh value observation: PCM feedback may advance its revision
//! between pages. Contiguous offsets, a fixed count and unique IDs bound the list;
//! these rows never promise a simultaneous snapshot or authorize a later edit.
use super::*;
use hex_audio::plugin_worker::{
    ControllerRequest, NativeWorkerControllerProgress, NativeWorkerControllerQuery,
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
    descriptor_signature: Option<[u8; 32]>,
    total: Option<u16>,
    rows: Vec<PluginParameterRow>,
    deadline: Instant,
    terminal: Option<Result<(), String>>,
    yield_to_control: bool,
}

impl Inventory {
    pub(super) fn request_yield(&mut self) {
        self.yield_to_control = true;
    }
    fn yield_between_pages(&mut self) -> bool {
        if self.yield_to_control && self.token.is_none() {
            self.terminal = Some(Err("vst3 parameter refresh yielded to control".into()));
            true
        } else {
            false
        }
    }
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

    fn accept_page(
        &mut self,
        first: u16,
        total: u16,
        revision: u64,
        descriptor_signature: [u8; 32],
        rows: Vec<PluginParameterRow>,
    ) -> Result<(), &'static str> {
        let count = usize::from(total);
        let offset = self.query.page_first.map_or(self.rows.len(), usize::from);
        if usize::from(first) != offset
            || offset.saturating_add(rows.len()) > count
            || self.total.is_some_and(|expected| expected != total)
            || self
                .descriptor_signature
                .is_some_and(|expected| expected != descriptor_signature)
            || (rows.is_empty() && offset < count)
            || rows
                .iter()
                .any(|row| self.rows.iter().any(|prior| prior.id == row.id))
        {
            return Err("vst3 parameter page differs");
        }
        self.rows.extend(rows);
        self.total = Some(total);
        self.revision = revision;
        self.descriptor_signature = Some(descriptor_signature);
        Ok(())
    }
}

impl Plugins {
    pub(in crate::native_audio) fn query_parameters(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        query: PluginParameterQuery,
    ) -> Result<(), String> {
        if let Some(first) = query.page_first {
            ControllerRequest::SelectedParameters { first }
                .validate()
                .map_err(|error| error.to_string())?;
        }
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
            || self.parameter_conversion.is_some()
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
            descriptor_signature: None,
            total: None,
            rows: Vec::new(),
            deadline: Instant::now() + Duration::from_millis(250),
            terminal: None,
            yield_to_control: false,
        });
        Ok(())
    }

    // Background display reads yield between pages to an actual user gesture.
    // Drain an admitted SDK token on its original owner before another query;
    // the Session still receives this read's explicit interrupted terminal.
    pub(super) fn yield_parameters_to_control(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        context: AudioSourceContext,
        cancellation: Option<&hex_session::service::RequestCancellation>,
    ) -> Result<(), String> {
        if let Some(inventory) = self.parameters.as_mut() {
            inventory.request_yield();
        }
        let deadline = Instant::now() + Duration::from_millis(150);
        while self
            .parameters
            .as_ref()
            .is_some_and(|inventory| inventory.terminal.is_none())
        {
            edits::check_cancel(cancellation)?;
            self.poll_parameters(control, Some(context));
            if self.parameters_ready() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("vst3 controller is still refreshing · try again".into());
            }
            edits::poll_control(control);
        }
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
        if let Ok(rows) = &rows {
            if inventory.query.page_first.is_some()
                && let Some(observed) = self.observed_parameters.iter_mut().find(|observed| {
                    observed.binding == inventory.binding
                        && observed.query.context == inventory.query.context
                        && observed.query.device == inventory.query.device
                        && observed.query.output_identity == inventory.query.output_identity
                })
            {
                for row in rows {
                    if let Some(old) = observed.rows.iter_mut().find(|old| old.id == row.id) {
                        *old = row.clone();
                    } else if observed.rows.len() < usize::from(inventory.total.unwrap_or(0)) {
                        observed.rows.push(row.clone());
                    }
                }
            } else {
                self.observed_parameters
                    .retain(|observed| observed.query.device != inventory.query.device);
                self.observed_parameters.push(ObservedInventory {
                    query: inventory.query,
                    binding: inventory.binding,
                    rows: rows.clone(),
                });
            }
        }
        Some(PluginParameterResult {
            query: inventory.query,
            source: rows.as_ref().ok().map(|_| inventory.binding),
            total: inventory.total.unwrap_or(0),
            descriptor_signature: rows.as_ref().ok().and(inventory.descriptor_signature),
            rows,
        })
    }

    pub(super) fn poll_parameters(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        context: Option<AudioSourceContext>,
    ) {
        // One bounded inventory per currently accepted source/device. Retiring
        // workers and replaced documents never retain display capabilities.
        self.observed_parameters.retain(|observed| {
            context.is_some_and(|context| {
                context.project_generation == observed.query.context.project_generation
            }) && control.output_identity() == Some(observed.query.output_identity)
                && self.sources.iter().any(|source| {
                    source.binding == observed.binding && source.accepted && !source.retiring
                })
        });
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
        if inventory.yield_between_pages() {
            return;
        }
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
            let Ok(first) = inventory
                .query
                .page_first
                .map_or_else(|| u16::try_from(inventory.rows.len()), Ok)
            else {
                inventory.terminal = Some(Err("vst3 parameter inventory exceeds its limit".into()));
                return;
            };
            match control.begin_native_source_controller_query_on_control(
                inventory.binding,
                ControllerRequest::SelectedParameters { first },
            ) {
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
                if inventory.yield_to_control {
                    inventory.terminal =
                        Some(Err("vst3 parameter refresh yielded to control".into()));
                    return;
                }
                let rows = page
                    .as_slice()
                    .iter()
                    .map(|parameter| {
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
                            editor_sequence: parameter.editor_sequence,
                        }
                    })
                    .collect();
                if let Err(error) = inventory.accept_page(
                    page.first,
                    page.total,
                    page.revision,
                    page.descriptor_signature,
                    rows,
                ) {
                    inventory.terminal = Some(Err(error.into()));
                    return;
                }
                if inventory.query.page_first.is_some()
                    || inventory.rows.len() == usize::from(page.total)
                {
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
                page_first: None,
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
            descriptor_signature: None,
            total: None,
            rows: Vec::new(),
            deadline: now + Duration::from_millis(250),
            terminal: None,
            yield_to_control: false,
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

    fn row(id: u32, revision: u64) -> PluginParameterRow {
        PluginParameterRow {
            id,
            name: format!("parameter {id}"),
            value: "0%".into(),
            normalized: 0.0,
            default_normalized: 0.0,
            steps: 0,
            editable: true,
            automation_class: None,
            page_first: 0,
            metadata_revision: revision,
            editor_sequence: 0,
        }
    }

    #[test]
    fn selected_pages_survive_feedback_revision_changes() {
        let now = Instant::now();
        let mut inventory = inventory(now);
        inventory
            .accept_page(0, 6, 3, [1; 32], (0..4).map(|id| row(id, 3)).collect())
            .unwrap();
        // PCM feedback may advance the observation revision between fresh
        // page reads. The inventory stays ordered and is complete at rev 4.
        inventory
            .accept_page(4, 6, 4, [1; 32], (4..6).map(|id| row(id, 4)).collect())
            .unwrap();
        assert_eq!(inventory.revision, 4);
        assert_eq!(
            inventory.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn selected_pages_refuse_gaps_count_changes_and_duplicate_ids() {
        let now = Instant::now();
        let mut gap = inventory(now);
        gap.accept_page(0, 3, 1, [1; 32], vec![row(1, 1)]).unwrap();
        assert_eq!(
            gap.accept_page(2, 3, 2, [1; 32], vec![row(3, 2)]),
            Err("vst3 parameter page differs")
        );

        let mut count = inventory(now);
        count
            .accept_page(0, 3, 1, [1; 32], vec![row(1, 1)])
            .unwrap();
        assert_eq!(
            count.accept_page(1, 4, 2, [1; 32], vec![row(2, 2)]),
            Err("vst3 parameter page differs")
        );

        let mut duplicate = inventory(now);
        duplicate
            .accept_page(0, 3, 1, [1; 32], vec![row(7, 1)])
            .unwrap();
        assert_eq!(
            duplicate.accept_page(1, 3, 2, [1; 32], vec![row(7, 2)]),
            Err("vst3 parameter page differs")
        );
        assert_eq!(
            duplicate.rows.len(),
            1,
            "failed page must not append partial data"
        );
        let mut changed = inventory(now);
        changed
            .accept_page(0, 6, 1, [1; 32], (0..4).map(|id| row(id, 1)).collect())
            .unwrap();
        assert_eq!(
            changed.accept_page(4, 6, 2, [2; 32], vec![row(4, 2), row(5, 2)]),
            Err("vst3 parameter page differs")
        );
        assert_eq!(
            changed.rows.len(),
            4,
            "descriptor changes never mix inventories"
        );
    }

    #[test]
    fn cached_views_read_one_live_page_and_keep_other_device_observations() {
        let now = Instant::now();
        let mut plugins = Plugins::default();
        let mut first = inventory(now);
        first.query.page_first = Some(0);
        let query = first.query;
        let binding = first.binding;
        first.rows = (1..=4).map(|id| row(id, 1)).collect();
        first.total = Some(6);
        first.descriptor_signature = Some([1; 32]);
        first.terminal = Some(Ok(()));
        plugins.parameters = Some(first);
        let result = plugins.take_parameters().unwrap();
        assert_eq!(result.source, Some(binding));
        assert_eq!(result.total, 6);
        assert_eq!(result.descriptor_signature, Some([1; 32]));

        // A second device must not evict the first device's gesture evidence.
        let mut second = inventory(now);
        let mut project =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 1).unwrap();
        for _ in 0..2 {
            project
                .execute(hex_project::Command::InsertMainDevice {
                    before: None,
                    state: hex_engine::default_native_device_state(
                        hex_engine::DeviceTypeId::from_static("hex.utility"),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        second.query.device = project.project().main_devices()[1].id();
        second.rows = vec![row(8, 2)];
        second.total = Some(1);
        second.terminal = Some(Ok(()));
        plugins.parameters = Some(second);
        plugins.take_parameters().unwrap();
        assert_eq!(plugins.observed_parameters.len(), 2);

        let mut selected = inventory(now);
        selected.query = PluginParameterQuery {
            page_first: Some(4),
            ..query
        };
        selected
            .accept_page(4, 6, 3, [1; 32], vec![row(9, 3), row(10, 3)])
            .unwrap();
        selected.terminal = Some(Ok(()));
        plugins.parameters = Some(selected);
        let result = plugins.take_parameters().unwrap();
        assert_eq!(result.total, 6);
        assert_eq!(result.rows.unwrap().len(), 2);
        let observed = plugins
            .observed_parameters
            .iter()
            .find(|item| item.query.device == query.device)
            .unwrap();
        assert_eq!(
            observed.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 9, 10]
        );
        assert_eq!(plugins.observed_parameters.len(), 2);
    }

    #[test]
    fn user_control_retires_a_background_read_without_publishing_partial_rows() {
        let mut plugins = Plugins::default();
        let mut pending = inventory(Instant::now());
        pending
            .accept_page(0, 6, 1, [1; 32], (0..4).map(|id| row(id, 1)).collect())
            .unwrap();
        pending.yield_to_control = true;
        assert!(pending.yield_between_pages());
        plugins.parameters = Some(pending);
        assert!(plugins.parameters_ready());
        let result = plugins.take_parameters().unwrap();
        assert_eq!(
            result.rows,
            Err("vst3 parameter refresh yielded to control".into())
        );
        assert_eq!(result.source, None);
        assert_eq!(result.descriptor_signature, None);
        assert!(plugins.observed_parameters.is_empty());
    }

    #[test]
    fn opening_the_plugin_editor_yields_an_automatic_inventory_read() {
        let pending = inventory(Instant::now());
        let binding = pending.binding;
        let device = pending.query.device.value();
        let generation = pending.query.context.project_generation;
        let mut plugins = Plugins::default();
        plugins.sources.push(Source {
            binding,
            owner: ProcessorChainOwner::Main,
            device,
            project_generation: generation,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: false,
            editor_open: false,
            editor_transfer: false,
            editor_open_on_insert: false,
            edit_backlog: false,
            edit_recovery: None,
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        });
        plugins.parameters = Some(pending);
        plugins.toggle_editor(generation, device).unwrap();
        assert!(
            matches!(plugins.editor, Some(super::super::editor::EditorOperation::Pending { binding: current, .. }) if current == binding)
        );
        assert!(plugins.parameters.as_mut().unwrap().yield_between_pages());
        let result = plugins.take_parameters().unwrap();
        assert_eq!(
            result.rows,
            Err("vst3 parameter refresh yielded to control".into())
        );
        assert!(
            plugins.sources[0].accepted,
            "display cancellation never retires the source"
        );
    }
}
