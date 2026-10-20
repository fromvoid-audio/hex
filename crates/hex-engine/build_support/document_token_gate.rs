//! The sealed Storage FileToken constructor family, separately from device
//! state: global Arc<[u8]>::from(Vec<u8>) AND Arc<[u8]>::from(&[u8]).
//! Pinned alloc sync.rs: Vec -> allocate_for_slice_in; u8 slice specialization
//! -> copy_from_slice -> allocate_for_slice; both -> allocate_for_layout ->
//! arcinner_layout_for_value_layout. No raw/custom/Weak constructor ingress.
pub const FAMILY: &str = "storage-file-token-vec-and-copy-slice-arc-v1";
pub const SOURCE_SHA256: &str = "ce15d00d92133cf1f55cd1905d7fe6b4e23e62090dabff6c68fae2d7fe78fe8f";
pub fn decide(observed: super::pcm_gate::Observation<'_>) -> Result<(), &'static str> {
    // Only the environmental selector is shared, never the other family label
    // or a later inventory's result. Constructor proof is pinned above.
    super::pcm_gate::decide(observed)
}
#[cfg(test)]
mod tests {
    #[test]
    fn file_token_family_requires_actual_supported_pinned_install_and_selector() {
        use super::super::pcm_gate::{COMMIT, Observation};
        for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
            let verbose = format!("release: 1.94.0\ncommit-hash: {COMMIT}\nhost: {target}\n");
            for (installed, flags, accepted) in [
                (true, false, true),
                (false, false, false),
                (true, true, false),
            ] {
                assert_eq!(
                    super::decide(Observation {
                        verbose: &verbose,
                        host: target,
                        target,
                        ordinary_install: installed,
                        unsupported_inputs: flags,
                    })
                    .is_ok(),
                    accepted
                );
            }
            assert!(
                super::decide(Observation {
                    verbose: &verbose,
                    host: target,
                    target: "wasm32-unknown-unknown",
                    ordinary_install: true,
                    unsupported_inputs: false,
                })
                .is_err()
            );
            let wrong = verbose.replace(COMMIT, "foreign-compiler");
            assert!(
                super::decide(Observation {
                    verbose: &wrong,
                    host: target,
                    target,
                    ordinary_install: true,
                    unsupported_inputs: false,
                })
                .is_err()
            );
        }
        assert_ne!(
            super::FAMILY,
            "canonical-native-state-vec-slices-empty-array-v1"
        );
        assert_eq!(super::SOURCE_SHA256.len(), 64);
    }
}
