//! Actual signed Root capability publication, with no canonical edit or graph
//! replacement. Caller-owned query facts are disposed before publication.
use super::*;
use crate::{
    plugin_effects::{AuthoredSelection, CapturedCanonicalBases, Hosting},
    plugin_worker::{
        NativeWorkerAuthoredParameterProgress as Progress, NativeWorkerControllerProgress,
        NativeWorkerControllerQuery,
    },
};
use hex_plugin_host::protocol::ParameterPageQuery;

fn wait<T>(mut run: impl FnMut() -> Result<T, &'static str>) -> T {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match run() {
            Ok(value) => return value,
            Err(
                "native preparation is busy"
                | "native controller mailbox is busy"
                | "native controller query is already pending"
                | "native source is busy",
            ) => {}
            Err(error) => panic!("native extension: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}

fn facts(
    f: &mut OrdinaryFixture,
    host: &Hosting,
    source: AppliedAudioSource,
    initial: &NativeDeviceState,
    ids: &[u32],
    class: [u8; 16],
) -> CapturedCanonicalBases {
    let selected = AuthoredSelection::from_targets(ids.iter().map(|&id| (id, Some(class))))
        .unwrap()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let mut capture = loop {
        match host.begin_canonical_bases_capture_on_control(
            &mut f.control,
            source,
            f.binding,
            initial,
            &selected,
            true,
        ) {
            Ok(capture) => break capture,
            Err(error)
                if matches!(
                    error.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) => {}
            Err(error) => panic!("isolated constructor: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    loop {
        if let Some(result) = capture.poll_on_control(&mut f.control, &mut || false) {
            return result.unwrap();
        }
        assert!(
            Instant::now() < until,
            "isolated constructor query must dispose its scratch"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn observed(f: &mut OrdinaryFixture) -> ParameterPageQuery {
    let token = wait(|| {
        f.control.begin_native_source_controller_query_on_control(
            f.binding,
            ParameterPageQuery::default(),
        )
    });
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match f
            .control
            .poll_native_source_controller_query_on_control(f.binding, &token)
        {
            Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {}
            Ok(NativeWorkerControllerProgress::Page(page)) => {
                return ParameterPageQuery {
                    first: page.first,
                    revision: page.revision,
                    conversion: None,
                };
            }
            other => panic!("source page: {other:?}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}

fn changed_canonical(
    f: &mut OrdinaryFixture,
    host: &Hosting,
    source: AppliedAudioSource,
    initial: &NativeDeviceState,
) -> NativeDeviceState {
    let until = Instant::now() + Duration::from_secs(3);
    let mut capture = loop {
        match host.begin_manual_base_capture_on_control(
            &mut f.control,
            source,
            f.binding,
            initial,
            7,
            0.75,
        ) {
            Ok(capture) => break capture,
            Err(error)
                if matches!(
                    error.as_str(),
                    "native preparation is busy" | "native source is busy"
                ) => {}
            Err(error) => panic!("isolated changed canonical: {error}"),
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    };
    loop {
        if let Some(result) = capture.poll_on_control(&mut f.control, &mut || false) {
            return result.unwrap();
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn complete(
    f: &mut OrdinaryFixture,
    token: &NativeWorkerControllerQuery,
    facts: &CapturedCanonicalBases,
) -> Result<Progress, &'static str> {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match f
            .control
            .complete_native_source_parameter_extension_on_control(f.binding, token, facts)
        {
            Ok(Progress::Pending) | Err("native preparation is busy") => {}
            other => return other,
        }
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}

fn capabilities(f: &OrdinaryFixture, initial: &NativeDeviceState) -> (u64, Vec<(u32, u32, u64)>) {
    let device = f.editor.project().main_devices()[0].id();
    wait(|| {
        f.control
            .with_native_effect_definition(&[f.binding], |definition| {
                use hex_lattice::NativeDeviceDefinition;
                let context = hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                };
                let source = definition
                    .foreign_automation_source(
                        ProcessorInstanceIdentity::new(ProcessorChainOwner::Main, device.value())
                            .unwrap(),
                        initial,
                        &context,
                    )
                    .unwrap()
                    .unwrap();
                Ok((
                    source.revision.get(),
                    source
                        .parameters
                        .iter()
                        .map(|p| (p.id, p.steps, p.normalized_base_bits))
                        .collect(),
                ))
            })
            .map_err(|_| "native preparation is busy")
    })
}

#[test]
#[ignore = "explicit source-bound native instance and original SDK fixtures required"]
fn original_root_extends_only_acknowledged_targets_with_isolated_constructor_bases() {
    if std::env::var_os("HEX_ROOT_PARAMETER_EXTENSION_CHILD").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("HEX_ROOT_PARAMETER_EXTENSION_CHILD", "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "144")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let _serial = serial();
    for initially_armed in [false, true] {
        let mut f =
            fixture_with_authored("processing", 2, 8, if initially_armed { &[7] } else { &[] });
        let initial = f.control.native_source_prepared_state(f.binding).unwrap();
        let class = hex_plugin_state::decode(&initial).unwrap().identity.class;
        let old = capabilities(&f, &initial);
        assert_eq!(old.1.len(), usize::from(initially_armed));
        let (build, _, preparation) =
            crate::candidate_custody::native_source_test_fixture::input_for(
                "processing",
                class.bytes(),
            );
        let host = Hosting::from_test_build(build, preparation.limits);
        let root = f.control.native_candidate_root();
        let playback = compile(&f, true);
        let revision = publish(&mut f.control, playback, PublicationPolicy::InheritRuntime);
        let source = AppliedAudioSource {
            context: context(),
            audio_revision: revision,
            output_identity: f.control.output_identity().unwrap(),
        };
        f.control.command(PlaybackCommand::Play).unwrap();
        for count in 1..=3 {
            let pcm = block(&mut f);
            replacement::wait_for_blocks(&root, f.binding, count);
            if count == 3 {
                assert_eq!(pcm, [[0.25, -0.5]; 8]);
            }
        }
        let selected: &[u32] = if initially_armed { &[7, 11] } else { &[11] };
        let constructor = facts(&mut f, &host, source, &initial, selected, class.bytes());
        let changed = changed_canonical(&mut f, &host, source, &initial);
        let wrong_constructor = facts(&mut f, &host, source, &changed, selected, class.bytes());
        let q = observed(&mut f);
        let refused = wait(|| {
            f.control
                .begin_native_source_parameter_extension_on_control(f.binding, class, 11, 0, q)
        });
        assert!(
            complete(&mut f, &refused, &wrong_constructor).is_err(),
            "approved same-source current canonical facts cannot replace constructor bases"
        );
        assert_eq!(capabilities(&f, &initial), old);
        let q = observed(&mut f);
        let token = wait(|| {
            f.control
                .begin_native_source_parameter_extension_on_control(f.binding, class, 11, 0, q)
        });
        assert!(
            f.control
                .poll_native_source_controller_query_on_control(f.binding, &token)
                .is_err()
        );
        let wrong = Binding {
            epoch: f.binding.epoch + 1,
            ..f.binding
        };
        assert!(
            f.control
                .complete_native_source_parameter_extension_on_control(wrong, &token, &constructor)
                .is_err()
        );
        assert_eq!(
            capabilities(&f, &initial),
            old,
            "SDK query completion alone must not publish capabilities"
        );
        assert!(matches!(
            complete(&mut f, &token, &constructor).unwrap(),
            Progress::Armed {
                id: 11,
                steps: 0,
                ..
            }
        ));
        let new = capabilities(&f, &initial);
        assert!(
            new.0 > old.0,
            "changed program contents require a fresh source-issued key"
        );
        assert_eq!(new.1.len(), selected.len());
        assert!(old.1.iter().all(|old| new.1.contains(old)));
        assert_eq!(new.1.last(), Some(&(11, 0, 0.25_f64.to_bits())));
        assert!(
            f.control
                .complete_native_source_parameter_extension_on_control(
                    f.binding,
                    &token,
                    &constructor
                )
                .is_err()
        );
        assert_eq!(
            f.control.native_source_prepared_state(f.binding),
            Some(initial.clone())
        );
        assert_eq!(f.control.status.native_effect_source(), Some(source));
        assert_eq!(
            block(&mut f),
            [[0.25, -0.5]; 8],
            "capability publication cannot alter the playing processor"
        );
        replacement::wait_for_blocks(&root, f.binding, 4);
        let q = observed(&mut f);
        let token = wait(|| {
            f.control
                .begin_native_source_parameter_extension_on_control(f.binding, class, 11, 0, q)
        });
        let unchanged = capabilities(&f, &initial);
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match f
                .control
                .drain_native_source_parameter_extension_on_control(f.binding, &token)
            {
                Ok(Progress::Pending) | Err("native preparation is busy") => {}
                Ok(Progress::Armed { .. }) => break,
                other => panic!("drain: {other:?}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(
            capabilities(&f, &initial),
            unchanged,
            "draining cancellation cannot publish a new program key"
        );
        f.control.command(PlaybackCommand::Stop).unwrap();
        assert!(
            f.control
                .complete_native_source_parameter_extension_on_control(
                    f.binding,
                    &token,
                    &constructor
                )
                .is_err()
        );
        dispose(f);
    }
}
