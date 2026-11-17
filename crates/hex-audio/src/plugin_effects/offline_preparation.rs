//! Actual private offline sources held stopped while their one graph is compiled.
use super::*;
use hex_plugin_host::{
    native::Quiescent,
    protocol::{PresentationLatency, PresentationPlan},
};

/// One original offline SDK lease and its reserved next epoch. This preparation
/// owns no live Root slot and cannot publish a graph or callback endpoint. The
/// compiler borrows only its validated source-derived configuration; completion
/// resumes this same lease before creating the final paired worker/callback.
pub struct OfflineEffectPreparation {
    source: Quiescent,
    renewal: NativeEffectRenewal,
    configuration: Configuration,
    max_data_bytes: usize,
    bytes: usize,
    authored: Option<AuthoredSelection>,
    bypassed: bool,
}
impl OfflineEffectPreparation {
    pub(super) fn from_effect_on_control(
        effect: NativeEffect,
        max_data_bytes: usize,
        authored: Option<AuthoredSelection>,
        bypassed: bool,
    ) -> Result<Self, String> {
        let mut configuration = effect.callback.configuration.clone();
        let original_bytes = effect
            .constructor_storage_on_control()
            .and_then(|s| s.data_bytes())
            .ok_or("offline vst3 storage unavailable")?;
        let (source, renewal) = effect
            .into_quiescent_on_control()
            .map_err(|e| e.finish_on_control().to_string())?;
        if source.completed_blocks() != 0
            || source.configuration().binding.root != u64::MAX
            || source.facts().algorithmic_latency_frames
                != configuration.prepared.processor_latency_frames()
        {
            let reason = "offline vst3 preparation changed before graph planning";
            return Err(match source.cancel_and_reap_on_control() {
                Ok(_) => reason.into(),
                Err(cleanup) => format!("{reason} · {cleanup}"),
            });
        }
        configuration.binding.epoch = configuration
            .binding
            .epoch
            .checked_add(1)
            .ok_or("offline vst3 epoch exhausted")?;
        configuration.prepared = configuration
            .prepared
            .with_binding(configuration.binding)
            .map_err(|e| e.to_string())?;
        let bytes = source
            .retained_storage_on_control()
            .and_then(|s| s.data_bytes.checked_add(std::mem::size_of::<Self>()))
            .and_then(|bytes| {
                original_bytes
                    .checked_add(std::mem::size_of::<NativeEffect>())
                    .and_then(|original| original.checked_add(std::mem::size_of::<Self>()))
                    .map(|original| bytes.max(original))
            })
            .ok_or("offline vst3 storage unavailable")?;
        Ok(Self {
            source,
            renewal,
            configuration,
            max_data_bytes,
            bytes,
            authored,
            bypassed,
        })
    }
    pub(super) fn configuration(&self) -> Result<&Configuration, DeviceContractError> {
        let original = self.source.configuration().binding;
        let current = self.configuration.binding;
        if original != self.renewal.binding
            || original.root != u64::MAX
            || current.root != original.root
            || current.instance != original.instance
            || current.graph != original.graph
            || original.epoch.checked_add(1) != Some(current.epoch)
            || self.source.completed_blocks() != 0
        {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        Ok(&self.configuration)
    }
    pub fn prepared(&self) -> PreparedForeignProcessor {
        self.configuration.prepared
    }
    pub fn finite_tail_frames(&self) -> Option<u64> {
        match self.configuration.prepared.output_tail() {
            hex_lattice::OutputTail::Finite { max_tail_frames } => Some(max_tail_frames),
            hex_lattice::OutputTail::Unbounded => None,
        }
    }
    /// Conservative listed control storage across stopped and final owners;
    /// external plugin memory remains outside this DATA accounting.
    pub fn retained_data_bytes(&self) -> usize {
        self.bytes
    }
    pub fn prepare_revision_on_control(
        sources: &[Self],
        compile: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        let definition = NativeEffectDefinition::from_offline_preparations(sources)
            .map_err(|e| e.to_string())?;
        hex_engine::AudioCandidateEffects::with_host_definitions(&[&definition], compile)
            .map_err(|e| e.to_string())
    }
    fn presentation(
        &self,
        observation: &hex_engine::PreparedPlaybackObservation,
    ) -> Result<PresentationPlan, String> {
        if observation.sample_rate() != self.configuration.sample_rate {
            return Err("offline vst3 graph rate differs".into());
        }
        if self.bypassed {
            return Ok(PresentationPlan::AllowUnknown);
        }
        let Some(stage) = observation.processor_latency(
            self.configuration.identity.device_id(),
            hex_plugin_state::TYPE_ID,
        ) else {
            // Source-resource stages without a graph projection retain the old
            // explicit unknown mode; no physical/device value is guessed.
            return Ok(PresentationPlan::AllowUnknown);
        };
        if stage.stage_frames == 0 && self.prepared().total_latency_frames() != 0 {
            return Ok(PresentationPlan::AllowUnknown); // host-bypassed stage
        }
        if stage.stage_frames != self.prepared().total_latency_frames() {
            return Err("offline vst3 graph latency differs".into());
        }
        let output_frames = stage
            .downstream_frames
            .checked_add(self.prepared().bridge_latency_frames())
            .ok_or("offline vst3 presentation overflow")?;
        Ok(PresentationPlan::Known(PresentationLatency {
            input_frames: stage.upstream_frames,
            output_frames,
        }))
    }
    fn resume_on_control(
        self,
        presentation: PresentationPlan,
        is_cancelled: impl FnMut() -> bool,
    ) -> Result<
        (
            hex_plugin_host::native::Prepared,
            NativeEffectRenewal,
            usize,
        ),
        String,
    > {
        let instance = self
            .source
            .resume_with_plan_while_on_control(
                self.configuration.binding,
                presentation,
                is_cancelled,
            )
            .map_err(|e| e.to_string())?;
        Ok((instance, self.renewal, self.max_data_bytes))
    }
    /// Complete on control after the sole graph compilation and before any
    /// rendering. File boundaries contribute zero hardware frames; the copied
    /// host bridge belongs downstream of the SDK output, never its own DSP.
    pub fn complete_on_control(
        self,
        observation: &hex_engine::PreparedPlaybackObservation,
        is_cancelled: impl FnMut() -> bool,
    ) -> Result<NativeEffect, String> {
        let presentation = self.presentation(observation)?;
        let identity = self.configuration.identity;
        let authored = self.authored.clone();
        let (instance, renewal, limit) = self.resume_on_control(presentation, is_cancelled)?;
        NativeEffect::prepare_with_renewal_on_control(
            identity,
            instance,
            limit,
            authored.as_ref().map(AuthoredSelection::parameters),
            Some(renewal),
            authored
                .as_ref()
                .and_then(AuthoredSelection::dense_parameter),
        )
        .map_err(|e| e.finish_on_control().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::ProcessorChainOwner;
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    #[test]
    #[ignore = "explicit source-bound original SDK helpers required"]
    fn actual_offline_graph_ports_exclude_own_dsp_and_keep_original_zero_pcm_sources() {
        if std::env::var_os("HEX_AUDIO_OFFLINE_PORTS_CHILD").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                    "--nocapture",
                ])
                .env("HEX_AUDIO_OFFLINE_PORTS_CHILD", "1")
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "37")
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
        let receipt = PathBuf::from(std::env::var_os("HEX_VST3_MACOS_RECEIPT").unwrap());
        let module = receipt
            .parent()
            .unwrap()
            .join("hex-vst3-host.app/Contents/Resources/fixtures/processing.vst3");
        let cid = [
            0x48, 0x45, 0x58, 0x50, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
        ];
        let source = hex_plugin_host::native::Source::inspect_on_control(
            &module,
            "Contents/MacOS/fixture",
            hex_plugin_state::ClassId::new(cid).unwrap(),
        )
        .unwrap();
        let identity = source.identity();
        let hex = |v: [u8; 32]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let code = hex(identity.binary.executable_sha256);
        let info = hex(identity.binary.info_sha256);
        let mut component = b"HXPS".to_vec();
        component.extend(3_u32.to_le_bytes());
        component.extend(1_f32.to_le_bytes());
        component.extend(48000_f64.to_le_bytes());
        component.extend([2, 2, 0, 0]);
        component.extend([0; 8]);
        let state = hex_plugin_state::encode(hex_plugin_state::State {
            identity,
            component: Some(&component),
            controller: None,
        })
        .unwrap();
        let host = Hosting::inspect_fixture_on_control(&PathBuf::from(
            std::env::var_os("HEX_VST3_INSTANCE_RECEIPT").unwrap(),
        ))
        .unwrap();
        let mut editor = Editor::new(Project::new(48000).unwrap(), 32).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "source".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
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
                track,
                return_track: bus,
                gain: 1.0,
            })
            .unwrap();
        let owners = [
            ProcessorChainOwner::Track(track.value()),
            ProcessorChainOwner::Track(track.value()),
            ProcessorChainOwner::Return(bus.value()),
            ProcessorChainOwner::Main,
        ];
        let mut stages = Vec::new();
        for owner in owners {
            let command = match owner {
                ProcessorChainOwner::Track(_) => Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                },
                ProcessorChainOwner::Return(_) => Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state: state.clone(),
                },
                _ => Command::InsertMainDevice {
                    before: None,
                    state: state.clone(),
                },
            };
            let Created::Device(id) = editor.execute(command).unwrap() else {
                panic!("device")
            };
            stages.push(
                host.prepare_offline_effect_on_control(
                    EffectSelection {
                        module: &module,
                        executable: "Contents/MacOS/fixture",
                        cid,
                        executable_sha256: &code,
                        info_sha256: &info,
                    },
                    owner,
                    id.value(),
                    48000,
                    PluginProfile::Effect,
                    &state,
                    None,
                    false,
                )
                .unwrap(),
            );
        }
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let resources =
            hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
                &Default::default(),
                editor.project(),
                1_000_000,
                &cancel,
            )
            .unwrap();
        let mut compiles = 0;
        let revision =
            OfflineEffectPreparation::prepare_revision_on_control(&stages, &mut |effects| {
                compiles += 1;
                effects.prepare_registered_revision(
                    editor.project(),
                    &BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                    &Default::default(),
                    &resources,
                    0,
                    &cancel,
                    None,
                )
            })
            .unwrap();
        assert_eq!(compiles, 1);
        let observation =
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
        assert_eq!(observation.main_latency().unwrap().arrival_frames, 2060);
        for (mut stage, (input_frames, output_frames)) in
            stages
                .into_iter()
                .zip([(0, 2057), (515, 1542), (1030, 1027), (1545, 512)])
        {
            stage.bypassed = true;
            assert_eq!(
                stage.presentation(&observation).unwrap(),
                PresentationPlan::AllowUnknown
            );
            stage.bypassed = false;
            let plan = stage.presentation(&observation).unwrap();
            assert_eq!(
                plan,
                PresentationPlan::Known(PresentationLatency {
                    input_frames,
                    output_frames
                })
            );
            let child = stage.source.child_id_on_control();
            let binding = stage.configuration.binding;
            let (instance, _, _) = stage.resume_on_control(plan, || false).unwrap();
            assert_eq!(instance.child_id_on_control(), child);
            assert_eq!(instance.configuration().binding, binding);
            assert_eq!(
                instance.ready().presentation,
                hex_plugin_host::protocol::Presentation::Applied
            );
            let (stopped, captured) = instance
                .quiesce_retained_on_control()
                .unwrap()
                .capture_state_retained_on_control()
                .unwrap();
            let decoded = hex_plugin_state::decode(&captured).unwrap();
            let bytes = decoded.component.unwrap();
            assert_eq!(
                u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
                input_frames
            );
            assert_eq!(
                u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
                output_frames
            );
            assert_eq!(stopped.completed_blocks(), 0);
            if output_frames == 512 {
                let binding = stopped.configuration().binding;
                let error = stopped
                    .resume_with_plan_while_on_control(
                        Binding {
                            epoch: binding.epoch + 1,
                            ..binding
                        },
                        plan,
                        || true,
                    )
                    .err()
                    .unwrap();
                assert!(error.to_string().contains("cancel"));
            } else {
                let retired = stopped.retire_on_control().unwrap();
                assert!(retired.status.success());
                assert_eq!(retired.terminal.unwrap().blocks, 0);
            }
        }
    }
}
