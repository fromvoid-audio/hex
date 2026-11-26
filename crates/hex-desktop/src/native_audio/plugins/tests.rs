//! Unit tests for `Plugins` idleness, powered-off document eligibility, and
//! retired source slot reclamation.
use super::*;

mod idle_tests {
    use super::*;

    #[test]
    fn unusable_catalog_configuration_precedes_host_initialization() {
        let oversized = "a".repeat(4096);
        for home in [None, Some(oversized.as_str())] {
            let mut plugins = Plugins::default();
            plugins.initialize_for_home(home.map(std::path::Path::new));
            assert!(plugins.initialized);
            assert!(!plugins.wants_profile());
            assert!(!plugins.control_held());
            assert!(plugins.catalog.is_none());
            assert_eq!(
                plugins.unavailable.as_deref(),
                Some("vst3 catalog path unavailable · configure a valid home folder")
            );
        }
    }

    #[test]
    fn absolute_catalog_configuration_keeps_the_browser_path() {
        let home = std::path::Path::new("/temporary/hex-catalog-fixture");
        let browser =
            hex_preferences::browser_places_path(home).with_file_name("plugin-catalog.json");
        assert_eq!(catalog_path_from_home(home), Some(browser));
    }

    #[test]
    fn recording_source_observation_rejects_pending_retiring_and_replaced_bindings() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let mut insert = || {
            let hex_project::Created::Device(device) = editor
                .execute(hex_project::Command::InsertMainDevice {
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
            device
        };
        let device = insert();
        let other = insert();
        let mut plugins = Plugins::default();
        let binding = Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 7,
        };
        let target = hex_session::service::automation_draft::Vst3CaptureTarget {
            device,
            class: [9; 16],
            parameter: 0,
            binding,
        };
        plugins.sources.push(Source {
            binding,
            owner: ProcessorChainOwner::Main,
            device: device.value(),
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: false,
            retiring: false,
            queued: true,
            observes_latency: true,
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
        });
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].accepted = true;
        assert!(plugins.matches_capture_source(context, target));
        plugins.sources[0].retiring = true;
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].retiring = false;
        plugins.sources[0].binding.epoch += 1;
        assert!(!plugins.matches_capture_source(context, target));
        plugins.sources[0].binding = binding;
        assert!(!plugins.matches_capture_source(
            AudioSourceContext {
                project_generation: 2,
                ..context
            },
            target
        ));
        assert!(!plugins.matches_capture_source(
            context,
            hex_session::service::automation_draft::Vst3CaptureTarget {
                device: other,
                ..target
            }
        ));
    }

    #[test]
    fn stopped_editor_backlog_keeps_polling_busy_until_drained_or_output_closes() {
        let mut plugins = Plugins::default();
        plugins.sources.push(Source {
            binding: Binding {
                root: 1,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
            owner: ProcessorChainOwner::Main,
            device: 2,
            project_generation: 1,
            operation: SourceOperation::Reopen,
            authored: None,
            accepted: true,
            retiring: false,
            queued: true,
            observes_latency: true,
            editor_open: true,
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
        });
        assert!(!plugins.idle());
        plugins.sources[0].editor_open = false;
        assert!(plugins.idle());
        plugins.sources[0].edit_backlog = true;
        assert!(!plugins.idle());
        plugins.sources[0].edit_backlog = false;
        assert!(plugins.idle());
        plugins.sources[0].edit_backlog = true;
        plugins.output_closed();
        assert!(plugins.idle());
    }
}

mod powered_off_document_tests {
    use super::*;

    #[test]
    fn only_bootstrap_or_the_exact_incoming_document_can_declare_initial_off() {
        let boot = AudioSourceContext {
            project_generation: 0,
            editor_revision: 0,
        };
        let next = AudioSourceContext {
            project_generation: 1,
            editor_revision: 0,
        };
        assert!(off_document_preparation(false, boot, None));
        assert!(off_document_preparation(false, next, Some(boot)));
        assert!(!off_document_preparation(true, next, Some(boot)));
        assert!(!off_document_preparation(false, next, None));
        assert!(!off_document_preparation(
            false,
            AudioSourceContext {
                editor_revision: 1,
                ..boot
            },
            None
        ));
        assert!(!off_document_preparation(false, boot, Some(boot)));
        assert!(!off_document_preparation(false, boot, Some(next)));
        assert!(!off_document_preparation(
            false,
            AudioSourceContext {
                project_generation: 2,
                ..boot
            },
            Some(boot)
        ));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn active_capacity_control_bytes_fit_reservation() {
        let policy = crate::audio_policy::AudioPolicy::new().unwrap();
        let active = policy.plugins.active_capacity;
        let monitor_bytes = active
            * (std::mem::size_of::<Binding>()
                + std::mem::size_of::<hex_engine::PausedMonitorForeignBinding>());
        let control_bytes = active * std::mem::size_of::<Source>()
            + monitor_bytes
            + active * std::mem::size_of::<editor_close::Worker>()
            + std::mem::size_of::<Plugins>()
            + 8192
            + hex_audio::plugin_effects::EffectSourceMetadata::MAX_DATA_BYTES;
        assert!(
            control_bytes < crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES,
            "control_bytes={control_bytes} must stay under PLUGIN_CONTROL_DATA_BYTES={}",
            crate::audio_policy::PLUGIN_CONTROL_DATA_BYTES
        );
    }

    #[cfg(all(
        target_os = "macos",
        target_arch = "aarch64",
        feature = "allocation-probe"
    ))]
    #[test]
    fn paused_monitor_slot_check_preserves_reserved_scratch_headroom() {
        let policy = crate::audio_policy::AudioPolicy::new().unwrap();
        let scratch = policy.plugins.scratch_capacity;
        // Verify that reserving scratch headroom requires `count + scratch`
        // vacant slots in Root: when only `scratch` slots remain, requesting
        // 1 paused monitor slot fails while `scratch` slots remain available
        // for control-side captures (save/canonical_bases/gesture scratch).
        let _serial = hex_audio::worker_fixture::serial_output();
        let (mut output, backend) =
            hex_audio::worker_fixture::open_after_retirement(48_000, false).unwrap();
        let control = output.publication.control().unwrap();
        policy.configure(control).unwrap();
        control
            .configure_native_source_preparation(scratch, policy.plugins.source)
            .unwrap();
        assert!(control.has_vacant_native_source_slots_on_control(scratch));
        assert!(!control.has_vacant_native_source_slots_on_control(scratch + 1));
        drop(backend);
        output.publication.close();
    }
}

mod pool_reclamation_tests {
    use super::*;

    fn vst3_state(tag: u8) -> NativeDeviceState {
        NativeDeviceState::new(
            hex_engine::DeviceTypeId::from_static("hex.vst3"),
            1,
            vec![tag],
        )
        .unwrap()
    }

    fn provider(tag: u8) -> ProviderIdentity {
        ProviderIdentity {
            class: [tag; 16],
            executable_sha256: [tag; 32],
            info_sha256: [tag; 32],
        }
    }

    fn source_record(
        owner: ProcessorChainOwner,
        device: u64,
        epoch: u64,
        generation: u64,
        provider_tag: u8,
    ) -> Source {
        Source {
            binding: Binding {
                root: 1,
                instance: device,
                graph: epoch,
                epoch,
            },
            provider: Some(provider(provider_tag)),
            owner,
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
            open_editor_group: None,
            editor_retirement: Default::default(),
            pending_touch: None,
            #[cfg(feature = "allocation-probe")]
            observed_latency_receipt: false,
            #[cfg(feature = "allocation-probe")]
            attempts: 0,
        }
    }

    fn retire_for_project(
        plugins: &mut Plugins,
        project: &hex_project::Project,
        generation: u64,
        revision: u64,
    ) {
        let context = AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        };
        let vst3 = hex_engine::DeviceTypeId::from_static("hex.vst3");
        plugins.retire_absent_sources_for_context(context, |visit| {
            for track in project.tracks() {
                if let Some(inst) = track.instrument()
                    && inst.state().type_id() == vst3
                {
                    visit(
                        ProcessorChainOwner::Track(track.id().value()),
                        inst.id().value(),
                    );
                }
                for device in track.devices() {
                    if device.state().type_id() == vst3 {
                        visit(
                            ProcessorChainOwner::Track(track.id().value()),
                            device.id().value(),
                        );
                    }
                }
            }
            for bus in project.return_tracks() {
                for device in bus.devices() {
                    if device.state().type_id() == vst3 {
                        visit(
                            ProcessorChainOwner::Return(bus.id().value()),
                            device.id().value(),
                        );
                    }
                }
            }
            for device in project.main_devices() {
                if device.state().type_id() == vst3 {
                    visit(ProcessorChainOwner::Main, device.id().value());
                }
            }
            Ok(())
        });
    }

    #[test]
    fn retire_absent_catalog_sources_marks_removed_replaced_moved_duplicate_and_old_generation_slots()
     {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        let hex_project::Created::Track(track_a) = editor
            .execute(hex_project::Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let hex_project::Created::Track(track_b) = editor
            .execute(hex_project::Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        let hex_project::Created::Device(live_main) = editor
            .execute(hex_project::Command::InsertMainDevice {
                before: None,
                state: vst3_state(1),
            })
            .unwrap()
        else {
            panic!("main device");
        };
        let hex_project::Created::Device(moved_device) = editor
            .execute(hex_project::Command::InsertDevice {
                track: track_b,
                before: None,
                state: vst3_state(2),
            })
            .unwrap()
        else {
            panic!("track b device");
        };

        let mut plugins = Plugins {
            catalog: Some(std::path::PathBuf::from("/tmp/hex-test-catalog.json")),
            ..Plugins::default()
        };
        // 0: older project generation -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            1,
            1,
            1,
        ));
        // 1: removed device -> must retire
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 999, 2, 2, 1));
        // 2: stale donor-owner slot after cross-track move (device lives on track_b) -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_a.value()),
            moved_device.value(),
            3,
            2,
            2,
        ));
        // 3: duplicate older accepted slot for live_main -> must retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            4,
            2,
            1,
        ));
        // 4: current live_main -> must stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Main,
            live_main.value(),
            5,
            2,
            1,
        ));
        // 5: current moved_device on track_b -> must stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_b.value()),
            moved_device.value(),
            6,
            2,
            2,
        ));

        retire_for_project(&mut plugins, editor.project(), 2, 5);

        for retired_index in [0, 1, 2, 3] {
            assert!(
                !plugins.sources[retired_index].accepted && plugins.sources[retired_index].retiring,
                "slot {retired_index} should be retired"
            );
        }
        for live_index in [4, 5] {
            assert!(
                plugins.sources[live_index].accepted && !plugins.sources[live_index].retiring,
                "slot {live_index} should remain accepted"
            );
        }
    }

    #[test]
    fn catalog_less_retirement_still_drops_moved_duplicate_and_extra_provider_slots() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 16).unwrap();
        let hex_project::Created::Track(track_a) = editor
            .execute(hex_project::Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track a");
        };
        let hex_project::Created::Track(track_b) = editor
            .execute(hex_project::Command::AddTrack { name: "b".into() })
            .unwrap()
        else {
            panic!("track b");
        };
        let hex_project::Created::Device(moved_device) = editor
            .execute(hex_project::Command::InsertDevice {
                track: track_b,
                before: None,
                state: vst3_state(1),
            })
            .unwrap()
        else {
            panic!("track b device");
        };

        let mut plugins = Plugins::default();
        assert!(plugins.catalog.is_none());
        // 0: removed device of provider 1 (another live source of provider 1 exists) -> retire
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 900, 1, 1, 1));
        // 1: stale donor-owner after move -> retire
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_a.value()),
            moved_device.value(),
            2,
            1,
            1,
        ));
        // 2: live moved device on track_b (provider 1) -> stay accepted
        plugins.sources.push(source_record(
            ProcessorChainOwner::Track(track_b.value()),
            moved_device.value(),
            3,
            1,
            1,
        ));
        // 3 & 4: two removed devices of provider 2 -> index 3 retires, index 4 stays sole fallback
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 901, 4, 1, 2));
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 902, 5, 1, 2));

        retire_for_project(&mut plugins, editor.project(), 1, 4);

        assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
        assert!(plugins.sources[1].retiring && !plugins.sources[1].accepted);
        assert!(plugins.sources[2].accepted && !plugins.sources[2].retiring);
        assert!(plugins.sources[3].retiring && !plugins.sources[3].accepted);
        assert!(plugins.sources[4].accepted && !plugins.sources[4].retiring);
    }

    #[test]
    fn finish_retires_refused_insertions_and_superseded_same_device_slots() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 3,
        };
        let mut plugins = Plugins {
            installed_removals_checked: Some(context),
            ..Plugins::default()
        };
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 7, 1, 1, 1));

        let accepted_op = Arc::new(PluginEffectControl::default());
        let mut replacement = source_record(ProcessorChainOwner::Main, 7, 2, 1, 1);
        replacement.accepted = false;
        replacement.operation = SourceOperation::Insertion(Arc::clone(&accepted_op));
        let replacement_binding = replacement.binding;
        plugins.sources.push(replacement);
        plugins.pending = Some(Pending {
            operation: Arc::clone(&accepted_op),
            binding: replacement_binding,
            context,
            canonical_context: context,
            result: Some(Ok(vst3_state(1))),
            facts_ready_for_session: true,
            facts_observed: true,
            #[cfg(feature = "allocation-probe")]
            facts_attempt_from_session: false,
        });
        plugins.finish(&accepted_op, true);
        assert!(plugins.installed_removals_checked.is_none());
        assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
        assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);

        let refused_op = Arc::new(PluginEffectControl::default());
        let mut refused = source_record(ProcessorChainOwner::Main, 8, 3, 1, 1);
        refused.accepted = false;
        refused.operation = SourceOperation::Insertion(Arc::clone(&refused_op));
        let refused_binding = refused.binding;
        plugins.sources.push(refused);
        plugins.pending = Some(Pending {
            operation: Arc::clone(&refused_op),
            binding: refused_binding,
            context,
            canonical_context: context,
            result: Some(Ok(vst3_state(1))),
            facts_ready_for_session: true,
            facts_observed: true,
            #[cfg(feature = "allocation-probe")]
            facts_attempt_from_session: false,
        });
        plugins.finish(&refused_op, false);
        assert!(plugins.sources[2].retiring && !plugins.sources[2].accepted);
    }

    #[test]
    fn has_reclaimable_retiring_source_honors_editor_transfer_and_hold_custody() {
        let mut plugins = Plugins::default();
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 10, 1, 1, 1));
        assert!(!plugins.has_reclaimable_retiring_source());

        plugins.sources[0].accepted = false;
        plugins.sources[0].retiring = true;
        assert!(plugins.has_reclaimable_retiring_source());

        plugins.sources[0].editor_transfer = true;
        assert!(!plugins.has_reclaimable_retiring_source());
        plugins.sources[0].editor_transfer = false;
        assert!(plugins.has_reclaimable_retiring_source());

        let binding = plugins.sources[0].binding;
        plugins.editor_ops.push(editor::EditorPending {
            binding,
            generation: 1,
            action: editor::EditorAction::Detach,
            request: None,
            admission_deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
            retry_after: None,
        });
        assert!(plugins.editor_holds(binding));
        assert!(!plugins.has_reclaimable_retiring_source());
        plugins.editor_ops.clear();
        assert!(plugins.has_reclaimable_retiring_source());
    }

    #[test]
    fn repeated_add_remove_replace_and_undo_cycles_over_2x_active_capacity_never_accumulate_slots()
    {
        let active_capacity = 8;
        let mut plugins = Plugins {
            catalog: Some(std::path::PathBuf::from("/tmp/hex-test-catalog.json")),
            configured: true,
            ..Plugins::default()
        };
        plugins.sources.try_reserve_exact(active_capacity).unwrap();

        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 64).unwrap();
        let mut next_epoch = 1u64;

        // Run 2.5x active_capacity add -> replace -> remove -> undo -> redo cycles.
        for cycle in 0..(active_capacity * 5 / 2) {
            let hex_project::Created::Device(first) = editor
                .execute(hex_project::Command::InsertMainDevice {
                    before: None,
                    state: vst3_state(1),
                })
                .unwrap()
            else {
                panic!("first insert");
            };
            assert!(
                plugins.sources.len() < plugins.sources.capacity(),
                "cycle {cycle}: slot pool must not exhaust before insert"
            );
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                first.value(),
                next_epoch,
                1,
                1,
            ));
            next_epoch += 1;

            // Replace with a second device and verify the old slot retires.
            let hex_project::Created::Device(replaced) = editor
                .execute(hex_project::Command::ReplaceMainDevice {
                    replacing: first,
                    state: vst3_state(2),
                })
                .unwrap()
            else {
                panic!("replace insert");
            };
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                replaced.value(),
                next_epoch,
                1,
                2,
            ));
            next_epoch += 1;
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources[0].retiring && !plugins.sources[0].accepted);
            assert!(plugins.sources[1].accepted && !plugins.sources[1].retiring);
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 1);

            // Remove the replacement device and verify its slot retires.
            editor
                .execute(hex_project::Command::RemoveDevice(replaced))
                .unwrap();
            next_epoch += 1;
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources.iter().all(|source| source.retiring));
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 0);

            // Undo reintroduces the removed device as a fresh slot; redo retires it again.
            assert!(editor.undo());
            assert!(editor.project().device(replaced).is_some());
            assert!(plugins.sources.len() < plugins.sources.capacity());
            plugins.sources.push(source_record(
                ProcessorChainOwner::Main,
                replaced.value(),
                next_epoch,
                1,
                2,
            ));
            next_epoch += 1;

            assert!(editor.redo());
            assert!(editor.project().device(replaced).is_none());
            retire_for_project(&mut plugins, editor.project(), 1, next_epoch);
            assert!(plugins.sources.iter().all(|source| source.retiring));
            plugins.sources.retain(|source| !source.retiring);
            assert_eq!(plugins.sources.len(), 0);
        }
    }

    #[test]
    fn re_enabling_powered_off_main_or_return_without_worker_never_preserves_as_unavailable() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 4,
        };
        for owner in [ProcessorChainOwner::Main, ProcessorChainOwner::Return(3)] {
            let identity = NativeProcessorIdentity::new(owner, 42).unwrap();
            let plugins = Plugins {
                saved_off_document: Some((1, identity)),
                ..Plugins::default()
            };

            // Undo of power-off with power_on: None still triggers powering_on
            // when no accepted worker exists and the device becomes enabled.
            assert_eq!(
                plugins.effective_powering_on(context, true, Some(identity), None, |_| Some(false)),
                Some(identity)
            );
            // Staying bypassed does not power on.
            assert_eq!(
                plugins.effective_powering_on(context, true, Some(identity), None, |_| Some(true)),
                None
            );
            // An enabled device transitioning bypassed true -> false or coming
            // from PoweredOff without a worker must be refused rather than
            // classified as a preserved UnavailableEffect.
            assert!(Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                Some(identity),
                Some(true),
            ));
            assert!(Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                None,
                Some(true),
            ));
            // Unrelated edits on an already-enabled unavailable plugin remain
            // preserved when it was neither PoweredOff nor bypassed before.
            assert!(!Plugins::re_enabling_without_worker(
                true,
                owner,
                42,
                Some(false),
                None,
                Some(false),
            ));
            // Project Open (preparing_edit == false) still preserves unavailable plugins.
            assert!(!Plugins::re_enabling_without_worker(
                false,
                owner,
                42,
                Some(false),
                None,
                Some(true),
            ));
        }

        // Accepting a newly prepared edit worker clears saved_off_document.
        let identity = NativeProcessorIdentity::new(ProcessorChainOwner::Main, 42).unwrap();
        let mut plugins = Plugins {
            saved_off_document: Some((1, identity)),
            edit: Some(context),
            ..Plugins::default()
        };
        let mut fresh = source_record(ProcessorChainOwner::Main, 42, 7, 1, 1);
        fresh.accepted = false;
        fresh.operation = SourceOperation::Edit {
            context,
            predecessor: None,
        };
        plugins.sources.push(fresh);
        plugins.finish_edit(context, true);
        assert!(plugins.saved_off_document.is_none());
        assert!(plugins.sources[0].accepted && !plugins.sources[0].retiring);
    }

    #[test]
    fn accepted_live_worker_never_silently_downgrades_to_unavailable_effect() {
        let context = AudioSourceContext {
            project_generation: 1,
            editor_revision: 5,
        };
        let mut plugins = Plugins::default();
        // Without any live worker, a previously-unavailable enabled plugin
        // may still preserve its opaque state across unrelated edits.
        assert!(!plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));

        // Once an accepted non-retiring worker exists in self.sources, compile
        // must never silently downgrade the device to UnavailableEffect while
        // stranding the live worker.
        plugins
            .sources
            .push(source_record(ProcessorChainOwner::Main, 42, 1, 1, 1));
        assert!(plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));
        assert!(plugins.refuse_unavailable_fallback(
            true,
            context,
            ProcessorChainOwner::Main,
            42,
            true,
            Some(false),
            None,
            Some(false),
        ));
        // Offline compilation without an active audio control keeps preserving.
        assert!(!plugins.refuse_unavailable_fallback(
            false,
            context,
            ProcessorChainOwner::Main,
            42,
            false,
            Some(false),
            None,
            Some(false),
        ));
    }
}
