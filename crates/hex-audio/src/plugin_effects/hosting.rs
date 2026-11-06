//! App-worker composition of checked helper provenance, selected source identity
//! and the existing Root. No source/endpoint custody or registry escapes here.
use super::*;
use crate::prepared::PublicationControl;
use hex_engine::{AudioCandidateEffects, ProcessorChainOwner};
use hex_plugin_host::{
    native,
    protocol::{PresentationLatency, PresentationPlan},
};
use hex_plugin_scan::macos::{build_identity::InstanceBuild, package::InstancePackage};
use hex_plugin_state::{BinaryIdentity, ClassId};
use std::{path::Path, time::Duration};

/// Borrowed explicit catalog choice. Hashes identify the selected bytes rather
/// than permission or authenticity. Compare both hashes before native spawn;
/// Root revalidates the original inputs again before admission.
pub struct EffectSelection<'a> {
    pub module: &'a Path,
    pub executable: &'a str,
    pub cid: [u8; 16],
    pub executable_sha256: &'a str,
    pub info_sha256: &'a str,
}
impl EffectSelection<'_> {
    fn inspect_on_control(&self) -> Result<native::Source, String> {
        let expected = BinaryIdentity {
            executable_sha256: digest(self.executable_sha256)?,
            info_sha256: digest(self.info_sha256)?,
        };
        let source = native::Source::inspect_on_control(
            self.module,
            self.executable,
            ClassId::new(self.cid).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        if source.identity().binary != expected {
            return Err("selected vst3 source changed · scan the module again".into());
        }
        Ok(source)
    }
}

/// Caller-derived graph/device contributions. None uses the SDK's explicit
/// unknown mode. Buffer duration alone does not establish device latency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectPresentation {
    pub input_frames: u32,
    pub output_frames: u32,
}

/// Fixed audio-bus shape for the selected project device slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginProfile {
    Effect,
    Instrument,
}

/// Checked helper artifact held by the canonical worker. Actual native source,
/// worker, endpoint, cancellation and retirement owners remain in its output Root.
pub struct Hosting {
    build: InstanceBuild,
    limits: native::Limits,
}
impl Hosting {
    /// Prepare a private offline source. Its reserved root is never admitted
    /// by the live output. The owner must join the instance before publishing.
    pub fn prepare_offline_instrument_on_control(
        &self,
        selection: EffectSelection<'_>,
        owner: ProcessorChainOwner,
        device_id: u64,
        sample_rate: u32,
        state: &NativeDeviceState,
    ) -> Result<NativeEffect, String> {
        let identity =
            ProcessorInstanceIdentity::new(owner, device_id).map_err(|e| e.to_string())?;
        let pending = native::Pending::prepare_offline_restored_on_control(
            &self.build,
            selection.inspect_on_control()?,
            native::Preparation {
                binding: Binding {
                    root: u64::MAX,
                    instance: device_id,
                    graph: 1,
                    epoch: 1,
                },
                sample_rate: f64::from(sample_rate),
                quantum: MAX_FRAMES as u16,
                input_channels: 0,
                output_channels: 2,
                limits: self.limits,
            },
            state.clone(),
            native::PreparationCancellation::new_on_control().map_err(|e| e.to_string())?,
            native::TrustUserCode::Acknowledged,
        )
        .map_err(|e| e.to_string())?;
        // A private bounce has no device presentation and this source is
        // configured before the isolated downstream graph has been compiled.
        let instance = pending
            .complete_with_plan_on_control(PresentationPlan::AllowUnknown)
            .map_err(|e| e.to_string())?;
        let data_limit = Self::source_data_reservation()?.max_retained_data_bytes;
        NativeEffect::prepare_retained_with_data_limit_on_control(identity, instance, data_limit)
            .map_err(|e| e.finish_on_control().to_string())
    }
    /// Maximum payload for the single pending native capture response.
    pub const CAPTURE_RESPONSE_DATA_BYTES: usize = hex_plugin_state::MAX_ENVELOPE_BYTES;
    /// Inspect the saved envelope's copied class/binary identity. This resolves
    /// no file, grants no native source and never replaces opaque state.
    pub fn saved_identity_on_control(
        state: &NativeDeviceState,
    ) -> Result<hex_plugin_state::Identity, String> {
        hex_plugin_state::decode(state)
            .map(|state| state.identity)
            .map_err(|error| error.to_string())
    }
    /// Listed retained helper provenance, inline host metadata and one bounded
    /// captured state payload for the pending consumer response. Inventory
    /// map-node/allocator backing remains an opaque bounded obligation as in
    /// InstanceBuild; this is not a process-memory measurement.
    pub fn control_data_bytes(&self) -> Option<usize> {
        self.build
            .retained_storage()?
            .data_bytes
            .checked_add(std::mem::size_of::<Self>())?
            .checked_add(Self::CAPTURE_RESPONSE_DATA_BYTES)
    }
    /// Initial finite source profile. Listed control/transport DATA is capped
    /// at4MiB per source; opaque plugin/OS memory is not an RSS claim. Reserve
    /// both bounded helper/source file workspaces and protocol work before any
    /// load. Root compares actual source-specific quotes against these limits.
    pub fn source_data_reservation() -> Result<crate::NativeSourceDataReservation, String> {
        let retained = 4_usize * 1024 * 1024;
        let peak = usize::try_from(hex_plugin_scan::MAX_BINARY)
            .ok()
            .and_then(|file| file.checked_mul(2))
            .and_then(|files| retained.checked_add(files))
            .and_then(|bytes| bytes.checked_add(hex_plugin_host::protocol::MAX_WORKING_DATA_BYTES))
            .ok_or("native source reservation overflow")?;
        Ok(crate::NativeSourceDataReservation {
            max_retained_data_bytes: retained,
            max_construction_peak_data_bytes: peak,
        })
    }
    /// Enable production hosting from the running app or independently
    /// verified source-launch tools. Inspection precedes any helper startup.
    pub fn current_on_control() -> Result<Self, String> {
        Ok(Self::from_package(InstancePackage::current_on_control()?))
    }
    /// Explicit qualified package inspection for headless delivery consumers,
    /// retaining the running app's fixed layout and inventory requirements.
    pub fn inspect_package_on_control(app: &Path) -> Result<Self, String> {
        Ok(Self::from_package(InstancePackage::inspect(app)?))
    }
    /// Use the source-launch verifier with an explicit headless fixture app.
    #[cfg(feature = "allocation-probe")]
    pub fn inspect_source_launch_fixture_on_control(app: &Path) -> Result<Self, String> {
        Ok(Self::from_package(InstancePackage::inspect_source_launch(
            app,
        )?))
    }
    /// Nondefault headless integration control. Explicit original fixture
    /// receipts retain normal InstanceBuild validation; production discovery
    /// never calls this or falls back to an original bootstrap receipt.
    #[cfg(feature = "allocation-probe")]
    pub fn inspect_fixture_on_control(receipt: &Path) -> Result<Self, String> {
        Ok(Self {
            build: InstanceBuild::inspect(receipt)?,
            limits: native::Limits {
                startup: Duration::from_secs(3),
                exchange: Duration::from_secs(1),
                retirement: Duration::from_secs(3),
            },
        })
    }
    /// Explicit headless fixture metadata, read without native execution.
    #[cfg(feature = "allocation-probe")]
    pub fn inspect_fixture_binary(
        path: &Path,
        executable: &str,
        cid: [u8; 16],
    ) -> Result<([u8; 32], [u8; 32]), String> {
        let source = native::Source::inspect_on_control(
            path,
            executable,
            ClassId::new(cid).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let binary = source.identity().binary;
        Ok((binary.executable_sha256, binary.info_sha256))
    }
    fn from_package(package: InstancePackage) -> Self {
        Self {
            build: package.build,
            limits: native::Limits {
                startup: Duration::from_secs(30),
                exchange: Duration::from_secs(1),
                retirement: Duration::from_secs(3),
            },
        }
    }
    #[cfg(test)]
    pub(crate) fn from_test_build(build: InstanceBuild, limits: native::Limits) -> Self {
        Self { build, limits }
    }
    /// Begin an explicit user-selected effect, including restoration of its
    /// saved identity from a user-opened project, on a configured output.
    /// A catalog match remains copied metadata; this revalidates the files.
    /// Quantum comes from that output, never the UI. Root allocates the actual
    /// binding/cancellation for the captured owner/planned canonical device.
    #[allow(clippy::too_many_arguments)] // Complete selected source and destination.
    pub fn begin_on_control(
        &self,
        output: &mut PublicationControl,
        selection: EffectSelection<'_>,
        owner: ProcessorChainOwner,
        device_id: u64,
        graph: u64,
        sample_rate: u32,
        profile: PluginProfile,
        restore: Option<NativeDeviceState>,
    ) -> Result<Binding, String> {
        let quantum = output
            .plugin_callback_quantum_on_control()
            .ok_or("vst3 hosting requires the plugin output profile")?;
        let root = output
            .output_identity()
            .ok_or("native output is unavailable")?;
        let identity =
            ProcessorInstanceIdentity::new(owner, device_id).map_err(|error| error.to_string())?;
        let source = selection.inspect_on_control()?;
        output
            .begin_native_source_preparation(
                identity,
                graph,
                &self.build,
                source,
                native::Preparation {
                    // Initial request only; Root replaces this entire binding.
                    binding: Binding {
                        root,
                        instance: device_id,
                        graph,
                        epoch: 1,
                    },
                    sample_rate: f64::from(sample_rate),
                    quantum,
                    input_channels: match profile {
                        PluginProfile::Effect => 2,
                        PluginProfile::Instrument => 0,
                    },
                    output_channels: 2,
                    limits: self.limits,
                },
                restore,
                native::TrustUserCode::Acknowledged,
            )
            .map_err(str::to_owned)
    }
    /// Complete the original source with known contributions or explicit unknown.
    /// Ok acknowledges Root custody, including retained native refusal/disposal;
    /// inspect actual progress before constructing or publishing endpoints.
    pub fn complete_on_control(
        &self,
        output: &mut PublicationControl,
        binding: Binding,
        presentation: Option<EffectPresentation>,
    ) -> Result<(), String> {
        output
            .complete_native_source_preparation(
                binding,
                presentation.map_or(PresentationPlan::AllowUnknown, |p| {
                    PresentationPlan::Known(PresentationLatency {
                        input_frames: p.input_frames,
                        output_frames: p.output_frames,
                    })
                }),
            )
            .map_err(str::to_owned)
    }
    /// Borrow actual Root-held definitions through the ordinary compiler facade.
    /// No registry, source or publication authority escapes this synchronous call.
    pub fn prepare_on_control(
        output: &PublicationControl,
        bindings: &[Binding],
        unavailable: &[super::UnavailableEffect<'_>],
        prepare: &mut dyn FnMut(
            &AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        output.with_native_effect_definition(bindings, |definition| {
            let definition = definition
                .including_unavailable(unavailable)
                .map_err(|error| error.to_string())?;
            AudioCandidateEffects::with_host_definitions(&[&definition], prepare)
                .map_err(|error| error.to_string())
        })
    }

    /// Preserve validated opaque devices without claiming native processing.
    /// The prepared graph contains identity placeholders, not foreign factories.
    /// Callers must expose their absence through the applied graph observation.
    pub fn prepare_unavailable_on_control(
        unavailable: &[super::UnavailableEffect<'_>],
        prepare: &mut dyn FnMut(
            &AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String> {
        let definition =
            NativeEffectDefinition::unavailable(unavailable).map_err(|error| error.to_string())?;
        AudioCandidateEffects::with_host_definitions(&[&definition], prepare)
            .map_err(|error| error.to_string())
    }
}

fn digest(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("invalid selected vst3 digest".into());
    }
    let mut bytes = [0; 32];
    for (dest, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let nibble = |byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err("invalid selected vst3 digest".to_owned()),
        };
        *dest = nibble(pair[0])? * 16 + nibble(pair[1])?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_identity_validation_precedes_source_inspection() {
        let valid = "1".repeat(64);
        for malformed in [
            String::new(),
            "a".repeat(63),
            "A".repeat(64),
            "é".repeat(32),
        ] {
            for wrong_executable in [false, true] {
                let selection = EffectSelection {
                    module: Path::new("/unopened-selection.vst3"),
                    executable: "Contents/MacOS/fixture",
                    cid: [1; 16],
                    executable_sha256: if wrong_executable { &malformed } else { &valid },
                    info_sha256: if wrong_executable { &valid } else { &malformed },
                };
                assert_eq!(
                    selection.inspect_on_control().unwrap_err(),
                    "invalid selected vst3 digest"
                );
            }
        }
        let selection = EffectSelection {
            module: Path::new("/unopened-selection.vst3"),
            executable: "Contents/MacOS/fixture",
            cid: [0; 16],
            executable_sha256: &valid,
            info_sha256: &valid,
        };
        assert_eq!(
            selection.inspect_on_control().unwrap_err(),
            "vst3 class identity is invalid"
        );
        assert_eq!(digest(&"ab".repeat(32)).unwrap(), [0xab; 32]);
    }
}
