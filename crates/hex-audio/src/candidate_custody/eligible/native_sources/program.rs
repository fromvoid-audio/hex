//! Original-source program revisions and worker observations on control.
//! Capture admission retains source/worker custody; copied keys grant no graph
//! permission. Reserved but unused keys may leave monotonic gaps.
use super::*;
impl Custody {
    pub(in crate::candidate_custody) fn reserve_native_program_revision(
        &self,
        binding: Binding,
    ) -> Result<std::num::NonZeroU64, &'static str> {
        self.with_live_capture(binding, |owner| match owner {
            Owner::Effect(effect) => effect.reserve_program_revision_on_control(),
            Owner::Loaned(control) => control.reserve_program_revision_on_control(),
            _ => Err("native automation program source is unavailable"),
        })
    }
}
impl CandidateRoot {
    pub(crate) fn reserve_native_program_revision(
        &self,
        binding: Binding,
    ) -> Result<std::num::NonZeroU64, &'static str> {
        self.require_native_source_control_open()?;
        let revision = self.0.eligible.reserve_native_program_revision(binding)?;
        self.require_native_source_control_open()?;
        Ok(revision)
    }
    pub(crate) fn native_source_received_program(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        self.with_native_source_endpoint(binding, |_, endpoint| {
            endpoint.received_program_on_control()
        })?
    }
    pub(crate) fn native_source_processed_program(
        &self,
        binding: Binding,
    ) -> Option<crate::plugin_worker::ReceivedProgram> {
        self.with_native_source_endpoint(binding, |_, endpoint| {
            endpoint.processed_program_on_control()
        })?
    }
}
#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;
    #[test]
    #[ignore = "explicit source-bound original SDK fixture required"]
    fn original_root_issues_program_revisions_through_endpoint_loan_and_reunion() {
        use hex_lattice::{DevicePreparationContext, NativeDeviceDefinition};
        const CHILD: &str = "HEX_ROOT_PROGRAM_REVISION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let name = std::thread::current().name().unwrap().to_owned();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--ignored", "--nocapture"])
                .env(CHILD, "1")
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "91")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            print!("{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        let outer = CandidateRoot::new(2).unwrap();
        outer
            .configure_domains(AudioCandidateDomainReservation {
                max_retained_data_bytes: 64 * 1024 * 1024,
                max_construction_peak_data_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        outer
            .configure_native_source_preparation(2, source_reservation())
            .unwrap();
        let root = &outer.0.eligible;
        let binding = start(root);
        assert!(
            outer.reserve_native_program_revision(binding).is_err(),
            "pending source has no revision grant"
        );
        root.complete_native_source(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
        let state = root.native_source_prepared_state(binding).unwrap();
        let class = hex_plugin_state::decode(&state).unwrap().identity.class;
        let selection =
            crate::plugin_effects::AuthoredSelection::from_targets([(7, Some(class.bytes()))])
                .unwrap()
                .unwrap();
        root.start_native_effect_with_selection(binding, Some(&selection))
            .unwrap();
        let context = DevicePreparationContext {
            sample_rate: 48_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        };
        let read = || {
            outer
                .with_native_effect_definition(&[binding], |definition| {
                    let source = definition
                        .foreign_automation_source(identity(), &state, &context)
                        .unwrap()
                        .unwrap();
                    Ok((
                        source.binding,
                        source.revision,
                        source.class,
                        source.parameters.to_vec(),
                    ))
                })
                .unwrap()
        };
        let original = read();
        assert_eq!(original.1.get(), binding.epoch);
        let first = outer.reserve_native_program_revision(binding).unwrap();
        assert_eq!(first.get(), original.1.get() + 1);
        let updated = read();
        assert_eq!(updated.0, original.0);
        assert_eq!(updated.1, first);
        assert_eq!(updated.2, original.2);
        assert_eq!(updated.3, original.3);
        assert_eq!(root.native_source_prepared_state(binding).unwrap(), state);
        assert_eq!(outer.native_source_received_program(binding), None);
        assert_eq!(outer.native_source_processed_program(binding), None);
        let callback = outer.loan_native_endpoint(binding).unwrap();
        let second = outer.reserve_native_program_revision(binding).unwrap();
        assert_eq!(second.get(), first.get() + 1);
        assert_eq!(read().1, second);
        let prepared = {
            let work = root.work.try_lock().unwrap();
            let Some(Owner::Loaned(control)) = work.sources.cells[0].owner.as_ref() else {
                panic!("original loaned worker")
            };
            control.prepared()
        };
        let mut bank = super::super::endpoints::NativeEndpointBank::new().unwrap();
        assert!(bank.install(binding.root, callback).is_ok());
        let node = hex_lattice::ForeignProcessorNode {
            owner: identity().owner(),
            device_id: identity().device_id(),
            device_type: state.type_id(),
            sample_rate: 48_000,
            configuration: &state,
            prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        };
        use hex_lattice::ForeignProcessors;
        let mut frames = [[0.5, -0.25]; 8];
        let render = || {
            bank.begin_rendering(8).unwrap();
            bank.process_segment(node, 0, &mut frames, None).unwrap();
            bank.finish_rendering(true).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let measured = allocation_counter::measure(render);
            assert_eq!(
                (
                    measured.count_total,
                    measured.bytes_total,
                    measured.count_current,
                    measured.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while outer.native_source_processed_program(binding).is_none() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let observed = outer.native_source_processed_program(binding).unwrap();
        assert_eq!(observed.binding, binding);
        assert_eq!(observed.sequence, 0);
        assert_eq!(
            observed.program, None,
            "a current compiler key is never substituted for an ordinary actual request"
        );
        let mut returned = std::array::from_fn(|_| None);
        assert!(bank.extract(binding.root, &mut returned));
        let callback = returned.into_iter().flatten().next().unwrap();
        outer
            .ingest_native_endpoint(callback)
            .unwrap_or_else(|_| panic!("original endpoint reunion"));
        assert_eq!(read().1, second);
        assert!(
            outer
                .reserve_native_program_revision(Binding {
                    epoch: binding.epoch + 1,
                    ..binding
                })
                .is_err()
        );
        assert_eq!(read().1, second);
        assert!(root.retire_native_source(binding));
        assert!(outer.reserve_native_program_revision(binding).is_err());
        assert_eq!(outer.native_source_received_program(binding), None);
        dispose(root, binding);
        outer.announce_retirement();
    }
}
