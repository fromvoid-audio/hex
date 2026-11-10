//! Copied VST3 controller inventory. The native sink owns every worker query.
use hex_engine::AudioSourceContext;
use hex_project::DeviceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginParameterQuery {
    pub context: AudioSourceContext,
    pub device: DeviceId,
    pub output_identity: u64,
    pub request_id: u64,
    /// None enumerates the inventory; Some reads one fresh bounded page.
    /// Page offsets are hints, never stable parameter or source identities.
    pub page_first: Option<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginParameterRow {
    pub id: u32,
    pub name: String,
    pub value: String,
    pub normalized: f64,
    pub default_normalized: f64,
    pub steps: u32,
    pub editable: bool,
    /// Class of the exact source that reported automation eligibility.
    pub automation_class: Option<[u8; 16]>,
    pub page_first: u16,
    pub metadata_revision: u64,
}

pub struct PluginParameterResult {
    pub query: PluginParameterQuery,
    /// Exact loaded source that supplied successful rows. Cached descriptions
    /// never acquire this stamp and cannot authorize a parameter gesture.
    pub source: Option<hex_lattice_contract::ForeignProcessorBinding>,
    pub total: u16,
    /// Stable complete-descriptor digest; changes invalidate cached hidden rows too.
    pub descriptor_signature: Option<[u8; 32]>,
    pub rows: Result<Vec<PluginParameterRow>, String>,
}

/// Versioned display-cache identity, independent of live values and host trims.
/// Admission still rechecks the actual installed binary, and each displayed
/// control must obtain its current capability/value from that loaded source.
pub fn vst3_parameter_cache_key(state: &hex_lattice_contract::NativeDeviceState) -> Option<String> {
    let identity = hex_plugin_state::decode(state).ok()?.identity;
    let mut key = String::with_capacity(163);
    key.push_str("v1-");
    for byte in identity
        .class
        .bytes()
        .into_iter()
        .chain(identity.binary.executable_sha256)
        .chain(identity.binary.info_sha256)
    {
        let hex = b"0123456789abcdef";
        key.push(char::from(hex[usize::from(byte >> 4)]));
        key.push(char::from(hex[usize::from(byte & 15)]));
    }
    Some(key)
}

/// Saved class identity only. Live automation eligibility belongs to the
/// loaded SDK worker and is checked again during candidate preparation.
pub fn vst3_automation_class(state: &hex_lattice_contract::NativeDeviceState) -> Option<[u8; 16]> {
    hex_plugin_state::decode(state)
        .ok()
        .map(|plugin| plugin.identity.class.bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_state::{BinaryIdentity, ClassId, Identity, State, encode};

    #[test]
    fn query_and_continuation_are_observations_without_canonical_or_status_changes() {
        use crate::service::{Request, Service, service_tests::next_event};
        let service = Service::new(48_000, 8, 8_000_000).unwrap();
        next_event(&service);
        service
            .submit(Request::Edit(hex_project::Command::InsertMainDevice {
                before: None,
                state: hex_engine::default_native_device_state(
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                )
                .unwrap(),
            }))
            .unwrap();
        let edited = next_event(&service);
        let hex_project::Created::Device(device) = edited.created.unwrap() else {
            panic!("device")
        };
        assert!(!edited.plugin_parameter_observation);
        let query = PluginParameterQuery {
            context: AudioSourceContext {
                project_generation: 0,
                editor_revision: edited.state.revision,
            },
            device,
            output_identity: 1,
            request_id: 1,
            page_first: Some(0),
        };
        service
            .submit(Request::QueryPluginParameters(query))
            .unwrap();
        let result = next_event(&service);
        assert!(result.plugin_parameter_observation);
        assert!(result.request_complete);
        assert!(result.status.is_none() && result.error.is_none());
        assert_eq!(result.state.revision, edited.state.revision);
        assert_eq!(result.state.project, edited.state.project);
        assert!(
            result.plugin_parameters.unwrap().rows.is_err(),
            "invalid plugin target stays a scoped read failure"
        );
        service.submit(Request::ContinuePluginParameters).unwrap();
        let continuation = next_event(&service);
        assert!(continuation.plugin_parameter_observation);
        assert!(!continuation.request_complete);
        assert!(continuation.status.is_none() && continuation.error.is_none());
        assert_eq!(continuation.state.revision, edited.state.revision);
    }

    #[test]
    fn cache_identity_tracks_class_and_binary_but_never_live_values() {
        let identity = Identity {
            class: ClassId::new([1; 16]).unwrap(),
            binary: BinaryIdentity {
                executable_sha256: [2; 32],
                info_sha256: [3; 32],
            },
        };
        let state = |identity, value| {
            encode(State {
                identity,
                component: Some(value),
                controller: None,
            })
            .unwrap()
        };
        let original = vst3_parameter_cache_key(&state(identity, &[1])).unwrap();
        assert_eq!(
            original,
            vst3_parameter_cache_key(&state(identity, &[2])).unwrap()
        );
        assert!(original.starts_with("v1-"));
        assert_eq!(original.len(), 163);
        for changed in [
            Identity {
                class: ClassId::new([4; 16]).unwrap(),
                ..identity
            },
            Identity {
                binary: BinaryIdentity {
                    executable_sha256: [4; 32],
                    ..identity.binary
                },
                ..identity
            },
            Identity {
                binary: BinaryIdentity {
                    info_sha256: [4; 32],
                    ..identity.binary
                },
                ..identity
            },
        ] {
            assert_ne!(
                original,
                vst3_parameter_cache_key(&state(changed, &[1])).unwrap()
            );
        }
    }
}
