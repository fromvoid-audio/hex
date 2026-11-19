//! Original topology custody at the closed Arrangement root. Runtime borrowers
//! refer to this same allocation; equal plans or pointer counts cannot adopt one.
use super::*;
use crate::{
    audio_storage::{Location, Owner},
    signal_graph::SignalGraph,
};
impl ArrangementRootAssembly {
    fn actual_signal<'a>(
        &'a self,
        external: Option<&'a Project>,
    ) -> Result<Option<&'a Arc<SignalGraph>>, AudioStorageError> {
        let installed = self
            .project
            .as_ref()
            .or(external)
            .and_then(|p| p.signal_graph.as_ref());
        if installed.is_some() && self.signal_graph.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(installed.or(self.signal_graph.as_ref()))
    }
    pub(super) fn signal_data_bytes(
        &self,
        external: Option<&Project>,
    ) -> Result<usize, AudioStorageError> {
        let Some(graph) = self.actual_signal(external)? else {
            return Ok(0);
        };
        let (p, o) = self
            .signal_declaration
            .as_ref()
            .and_then(|d| d.parts(graph))
            .ok_or(AudioStorageError::UnknownStorage)?;
        p.checked_add(o)
            .and_then(|n| n.checked_add(graph.nested_bytes()?))
            .ok_or(AudioStorageError::Overflow)
    }
    pub(in crate::audio_preparation) fn prepare_signal_graph(
        &mut self,
        canonical: &hex_project::Project,
    ) -> Result<(), AudioStorageError> {
        if !self.signal_reserved
            || self.signal_graph.is_some()
            || self.signal_declaration.is_some()
            || self.signal_disposal != Disposal::Unstarted
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let project = self
            .project
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if project.signal_graph.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.signal_handle = crate::audio_only_inputs::handle()?;
        let feeds = canonical
            .authored_routing()
            .iter()
            .map(|feed| {
                let source = canonical
                    .return_tracks()
                    .iter()
                    .position(|r| r.id() == feed.source())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let destination = canonical
                    .return_tracks()
                    .iter()
                    .position(|r| r.id() == feed.destination())
                    .ok_or(AudioStorageError::SourceMismatch)?;
                Ok((source, destination, feed.gain()))
            })
            .collect::<Result<Vec<_>, AudioStorageError>>()?;
        let graph = SignalGraph::prepare_owned(project, &feeds, canonical)
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let (graph, declaration) = graph.into_paired_parts_on_control();
        self.signal_graph = Some(graph);
        self.signal_declaration = Some(declaration);
        let graph = self
            .signal_graph
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?;
        let (payload, overhead) = self
            .signal_declaration
            .as_ref()
            .and_then(|d| d.parts(graph))
            .ok_or(AudioStorageError::UnknownStorage)?;
        let bytes = payload
            .checked_add(overhead)
            .and_then(|n| n.checked_add(graph.nested_bytes()?))
            .ok_or(AudioStorageError::Overflow)?;
        if bytes > crate::compensation::MAX_COMPENSATION_BYTES {
            return Err(AudioStorageError::ReservationExceeded);
        }
        project.main_output_latency_frames =
            crate::main_chain::output_latency(graph.main_latency(), &project.main_device_chain)
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        project.signal_graph = self.signal_graph.take();
        Ok(())
    }
    pub(super) fn visit_signal_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a Project>,
    ) -> Result<(), AudioStorageError> {
        if let Some(graph) = self.actual_signal(external)? {
            ledger.shared(
                graph,
                self.signal_declaration
                    .as_ref()
                    .and_then(|d| d.parts(graph)),
                Owner::exclusive(graph),
                self.signal_handle,
                None,
            )?;
            ledger.bytes(
                Owner::exclusive(graph),
                Location("signal-graph-nested", 0),
                graph.nested_bytes().ok_or(AudioStorageError::Overflow)?,
            )?;
        }
        Ok(())
    }
    pub(in crate::audio_preparation) fn dispose_signal_on_control(
        &mut self,
    ) -> Result<(), AudioStorageError> {
        if self.signal_disposal == Disposal::Pending
            || self
                .project
                .as_ref()
                .is_some_and(|p| p.signal_graph.is_some())
        {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.signal_disposal = Disposal::Pending;
        drop(self.signal_graph.take());
        self.signal_disposal = Disposal::Complete;
        self.signal_declaration = None;
        self.signal_handle = 0;
        self.signal_reserved = false;
        // Only an actual normal return opens reuse; failure cannot retry.
        self.signal_disposal = Disposal::Unstarted;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FeedTopology {
        Flat,
        GroupedSiblings,
        GroupSource,
        GroupDestination,
        ReturnSource,
    }
    #[test]
    fn flat_track_feed_constructor_accounts_for_graph_and_recovers_input() {
        track_feed_constructor_accounts_for_graph_and_recovers_input(FeedTopology::Flat);
    }
    #[test]
    fn grouped_track_feed_constructor_accounts_for_graph_and_recovers_input() {
        track_feed_constructor_accounts_for_graph_and_recovers_input(FeedTopology::GroupedSiblings);
    }
    #[test]
    fn group_output_feed_constructor_accounts_for_graph_and_recovers_input() {
        track_feed_constructor_accounts_for_graph_and_recovers_input(FeedTopology::GroupSource);
    }
    #[test]
    fn group_input_feed_constructor_accounts_for_graph_and_recovers_input() {
        track_feed_constructor_accounts_for_graph_and_recovers_input(
            FeedTopology::GroupDestination,
        );
    }
    #[test]
    fn return_feed_constructor_accounts_for_graph_and_recovers_input() {
        track_feed_constructor_accounts_for_graph_and_recovers_input(FeedTopology::ReturnSource);
    }
    fn track_feed_constructor_accounts_for_graph_and_recovers_input(topology: FeedTopology) {
        use hex_project::{Command, Created, Editor, RoutingOwner, TrackDeviceSlot};
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let (project, mut bank) = crate::audio_preparation::tests::fixture(1);
        let pointer = bank.sources.as_ptr();
        bank.reservation.allocation_items = 1024;
        bank.reservation.handle_obligations = 256;
        let mut editor = Editor::new(project, 16).unwrap();
        let source = editor.project().tracks()[0].id();
        let Created::Track(destination) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::RemoveInstrument { track: destination })
            .unwrap();
        editor
            .execute(Command::InsertDevice {
                track: source,
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    crate::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
        let mut feed_source = RoutingOwner::Track(source);
        let mut feed_destination = destination;
        if topology != FeedTopology::Flat && topology != FeedTopology::ReturnSource {
            let grouped = match topology {
                FeedTopology::GroupedSiblings => vec![source, destination],
                FeedTopology::GroupSource => vec![source],
                FeedTopology::GroupDestination => vec![destination],
                FeedTopology::Flat | FeedTopology::ReturnSource => Vec::new(),
            };
            let Created::Track(group) = editor
                .execute(Command::GroupTracks {
                    tracks: hex_project::TrackSelectionIds::new(&grouped).unwrap(),
                    name: "group".into(),
                })
                .unwrap()
            else {
                panic!("group")
            };
            if topology == FeedTopology::GroupSource {
                feed_source = RoutingOwner::Track(group);
            } else if topology == FeedTopology::GroupDestination {
                feed_destination = group;
            }
        }
        if topology == FeedTopology::ReturnSource {
            let Created::ReturnTrack(bus) = editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            editor
                .execute(Command::SetReturnSend {
                    track: source,
                    return_track: bus,
                    gain: 0.5,
                })
                .unwrap();
            editor
                .execute(Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: crate::default_native_device_state_for_class(
                        crate::NativeDeviceClass::Effect,
                        crate::DeviceTypeId::from_static("hex.limiter"),
                    )
                    .unwrap(),
                })
                .unwrap();
            feed_source = RoutingOwner::Return(bus);
        }
        editor
            .execute(Command::SetAudioFeed {
                source: feed_source,
                destination: feed_destination,
                gain: 0.5,
            })
            .unwrap();
        let registry = hex_devices::native_registry().unwrap();
        let quote = crate::AudioCandidateReservationQuote::for_project_with_effect_registry(
            editor.project(),
            1,
            &registry,
        )
        .unwrap();
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let token = custody
            .begin_input_on_control(
                crate::AudioArrangementInput::new(editor.project(), 11, bank)
                    .with_audio_prepared_revision()
                    .with_effect_registry(&registry),
            )
            .unwrap_or_else(|e| panic!("constructor: {:?}", e.reason));
        let revision = custody.candidate_root_view(&token).unwrap();
        let graph = revision.arrangement().signal_graph.as_ref().unwrap();
        assert!(graph.main_latency() > 0);
        assert_eq!(
            graph
                .tracks
                .iter()
                .map(|track| track.audio_feeds.len())
                .sum::<usize>()
                + graph
                    .returns
                    .iter()
                    .map(|bus| bus.audio_feeds.len())
                    .sum::<usize>(),
            1
        );
        let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 8,
                max_retained_bytes: usize::MAX,
                max_construction_peak_bytes: usize::MAX,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        let asset = editor.project().assets().next().unwrap().id();
        let media = std::collections::BTreeMap::from([(
            asset,
            crate::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap(),
        )]);
        let reference = Arc::new(
            crate::PreparedRevision::prepare(
                editor.project(),
                &media,
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut reference = crate::SessionPlayback::new(reference).unwrap();
        reference
            .visit_next_stream_chunk(32, None, &mut |_| Ok(()))
            .unwrap();
        reference
            .apply(crate::SessionPlaybackCommand::Play)
            .unwrap();
        owner
            .test_apply_action(crate::PlaybackAction::Transport(
                crate::PlaybackCommand::Play,
            ))
            .unwrap();
        let mut expected = [[0.0; 2]; 2048];
        let mut observed = expected;
        reference.render(&mut expected).unwrap();
        owner.test_render(&mut observed).unwrap();
        assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
        assert_eq!(observed, expected);
        owner.dispose_on_control().unwrap();
        assert_eq!(
            owner.recover_inputs_on_control().unwrap().sources.as_ptr(),
            pointer
        );
        let (_, mut cancelled_bank) = crate::audio_preparation::tests::fixture(1);
        let cancelled_pointer = cancelled_bank.sources.as_ptr();
        cancelled_bank.reservation.allocation_items = 1024;
        cancelled_bank.reservation.handle_obligations = 256;
        let mut cancelled = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let cancelled_token = cancelled
            .begin_input_on_control(
                crate::AudioArrangementInput::new(editor.project(), 12, cancelled_bank)
                    .with_audio_prepared_revision()
                    .with_effect_registry(&registry),
            )
            .unwrap_or_else(|e| panic!("constructor: {:?}", e.reason));
        assert!(cancelled.cancel(&cancelled_token));
        cancelled.cleanup_on_control();
        assert_eq!(
            cancelled
                .recover_on_control(&cancelled_token)
                .unwrap()
                .sources
                .as_ptr(),
            cancelled_pointer
        );

        let mut with_return = Editor::new(editor.project().clone(), 8).unwrap();
        with_return
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap();
        assert!(
            crate::AudioCandidateReservationQuote::for_project_with_effect_registry(
                with_return.project(),
                1,
                &registry,
            )
            .is_ok()
        );
    }
    #[test]
    fn equal_plan_and_layout_are_not_the_original_runtime_graph() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let (canonical, bank) = crate::audio_preparation::tests::fixture(1);
        let definitions = [];
        let registry = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
        let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
        let original = bank.sources.as_ptr();
        let token = custody
            .begin_input_on_control(
                crate::AudioArrangementInput::new(&canonical, 11, bank)
                    .with_audio_prepared_revision()
                    .with_effect_registry(&registry),
            )
            .ok()
            .unwrap();
        {
            let revision = custody.candidate_root_view(&token).unwrap();
            let project = revision.arrangement();
            let graph = project.signal_graph.as_ref().unwrap();
            let other = SignalGraph::prepare(project, &[], &canonical)
                .unwrap()
                .unwrap();
            assert_eq!(graph.as_ref(), other.as_ref());
            assert!(!Arc::ptr_eq(graph, &other));
            let declaration = custody
                .arrangement_clips
                .root
                .signal_declaration
                .as_ref()
                .unwrap();
            assert!(declaration.parts(graph).is_some());
            // This private receipt describes layout, not identity. Only the
            // private constructor can install the paired owner; the runtime
            // must independently match that actual original graph.
            assert_eq!(declaration.parts(graph), declaration.parts(&other));
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
                crate::AudioStorageLedgerReservation {
                    data_rows: 4096,
                    handle_rows: 2048,
                    root_rows: 8,
                    max_retained_bytes: usize::MAX,
                    max_construction_peak_bytes: usize::MAX,
                },
            )
            .unwrap();
            ledger
                .borrow_custody_on_control(&custody, crate::AudioStorageRole::Current)
                .unwrap();
            let runtime = crate::signal_graph::SignalRuntime::new(Arc::clone(graph)).unwrap();
            assert_eq!(
                runtime.visit_fresh_candidate_storage(&other, &mut ledger),
                Err(AudioStorageError::SourceMismatch)
            );
            runtime
                .visit_fresh_candidate_storage(graph, &mut ledger)
                .unwrap();
            assert_eq!(ledger.summary().unwrap().unknown_storage, 0);
        }
        assert!(custody.cancel(&token));
        custody.cleanup_on_control();
        assert_eq!(
            custody.recover_on_control(&token).unwrap().sources.as_ptr(),
            original
        );
    }
    #[test]
    fn claimed_return_and_main_tail_moves_retain_original_anchor_until_final_disposal() {
        use hex_lattice_contract::NativeDeviceMetadata;
        use hex_project::{Command, Created, Editor};
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        for main in [false, true] {
            for predecessor_has_chain in [false, true] {
                let (plain, _) = crate::audio_preparation::tests::fixture(1);
                let mut editor = Editor::new(plain.clone(), 8).unwrap();
                let definition = &hex_devices::utility::UTILITY_DEFINITION;
                let state = definition
                    .patch_parameter(
                        &definition.default_state().unwrap(),
                        hex_devices::utility::parameter::DC_REMOVE,
                        hex_lattice_contract::ParameterValue::Bool(true),
                    )
                    .unwrap();
                if main {
                    editor
                        .execute(Command::InsertMainDevice {
                            before: None,
                            state,
                        })
                        .unwrap();
                } else {
                    let Created::ReturnTrack(id) = editor
                        .execute(Command::AddReturnTrack { name: "bus".into() })
                        .unwrap()
                    else {
                        panic!("return")
                    };
                    let track = editor.project().tracks()[0].id();
                    editor
                        .execute(Command::SetReturnSend {
                            track,
                            return_track: id,
                            gain: 1.0,
                        })
                        .unwrap();
                    editor
                        .execute(Command::InsertReturnDevice {
                            return_track: id,
                            before: None,
                            state,
                        })
                        .unwrap();
                }
                let build = |project: &hex_project::Project, generation| {
                    let (_, mut bank) = crate::audio_preparation::tests::fixture(1);
                    bank.reservation.allocation_items = 1024;
                    bank.reservation.handle_obligations = 256;
                    let definitions = [];
                    let registry = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
                    let mut custody = crate::AudioPreparationCustody::try_new_on_control().unwrap();
                    let token = custody
                        .begin_input_on_control(
                            crate::AudioArrangementInput::new(project, generation, bank)
                                .with_audio_prepared_revision()
                                .with_effect_registry(&registry),
                        )
                        .unwrap_or_else(|refused| {
                            panic!("closed handoff fixture: {:?}", refused.reason)
                        });
                    let mut owner = crate::AudioCandidateOwner::try_new_on_control(custody, token)
                        .ok()
                        .unwrap();
                    owner
                        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                            data_rows: 4096,
                            handle_rows: 2048,
                            root_rows: 8,
                            max_retained_bytes: usize::MAX,
                            max_construction_peak_bytes: usize::MAX,
                        })
                        .unwrap();
                    owner.split_constructor_on_control().ok().unwrap()
                };
                let (mut old, old_anchor) = build(
                    if predecessor_has_chain {
                        editor.project()
                    } else {
                        &plain
                    },
                    11,
                );
                let (mut next, next_anchor) = build(
                    if predecessor_has_chain {
                        &plain
                    } else {
                        editor.project()
                    },
                    12,
                );
                old.apply_action(crate::PlaybackAction::Transport(
                    crate::PlaybackCommand::Play,
                ))
                .unwrap();
                old.render_with_node_signal_observer(&mut [[0.0; 2]; 4], None)
                    .unwrap();
                let old_identity = old.identity();
                let mut previous = Some(old);
                let mut retired = None;
                let mut manifest = None;
                next.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
                    .unwrap();
                assert!(previous.is_none());
                assert_eq!(retired.as_ref().unwrap().identity(), old_identity);
                assert!(next_anchor.matches_current(&next));
                next.render_with_node_signal_observer(&mut [[0.0; 2]; 4], None)
                    .unwrap();
                let mut anchors = vec![Some(old_anchor), Some(next_anchor)];
                retired
                    .take()
                    .unwrap()
                    .dispose_on_control()
                    .unwrap()
                    .record_anchor_cells_on_control(&mut anchors)
                    .unwrap();
                if predecessor_has_chain {
                    assert!(
                        anchors[0]
                            .as_mut()
                            .unwrap()
                            .finish_disposal_on_control()
                            .is_err(),
                        "actual retained tail keeps the original constructor anchor open"
                    );
                }
                assert_eq!(
                    manifest.as_ref().unwrap().moved_groups_from(old_identity),
                    usize::from(predecessor_has_chain)
                );
                if predecessor_has_chain {
                    let (mut third, third_anchor) = build(&plain, 13);
                    anchors.push(Some(third_anchor));
                    let mut previous = Some(next);
                    let mut retired = None;
                    let mut manifest = None;
                    third
                        .try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
                        .unwrap();
                    assert_eq!(
                        manifest.as_ref().unwrap().moved_groups_from(old_identity),
                        1,
                        "carried tail retains the first constructor origin"
                    );
                    retired
                        .take()
                        .unwrap()
                        .dispose_on_control()
                        .unwrap()
                        .record_anchor_cells_on_control(&mut anchors)
                        .unwrap();
                    assert!(
                        anchors[0]
                            .as_mut()
                            .unwrap()
                            .finish_disposal_on_control()
                            .is_err()
                    );
                    third
                        .render_with_node_signal_observer(&mut [[0.0; 2]; 4], None)
                        .unwrap();
                    next = third;
                }
                next.dispose_on_control()
                    .unwrap()
                    .record_anchor_cells_on_control(&mut anchors)
                    .unwrap();
                for anchor in anchors.iter_mut().flatten() {
                    anchor.finish_disposal_on_control().unwrap();
                    assert!(anchor.recover_inputs_on_control().is_some());
                }
            }
        }
    }
}
