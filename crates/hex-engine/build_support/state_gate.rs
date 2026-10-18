//! Separate sealed canonical NativeDeviceState constructor qualification.
//! Audited pinned std paths: Vec<u8> -> Arc<[u8]>,
//! Vec<DeviceAssetReference> -> Arc<[DeviceAssetReference]>, and fresh
//! Arc::new([u8; 0]) followed by allocation-preserving unsizing.
//! Environmental selector is shared; the PCM family label is not this proof.
pub const FAMILY: &str = "canonical-native-state-vec-slices-empty-array-v1";
pub fn decide(observed: super::pcm_gate::Observation<'_>) -> Result<(), &'static str> {
    super::pcm_gate::decide(observed)
}
#[cfg(test)]
mod tests {
    #[test]
    fn all_three_canonical_routes_require_supported_pinned_native_inputs() {
        for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
            let metadata = format!(
                "release: 1.94.0\ncommit-hash: {}\nhost: {target}\n",
                super::super::pcm_gate::COMMIT
            );
            for (installed, unsupported) in [(true, false), (false, false), (true, true)] {
                let result = super::decide(super::super::pcm_gate::Observation {
                    verbose: &metadata,
                    host: target,
                    target,
                    ordinary_install: installed,
                    unsupported_inputs: unsupported,
                });
                assert_eq!(result.is_ok(), installed && !unsupported);
            }
        }
    }
}
