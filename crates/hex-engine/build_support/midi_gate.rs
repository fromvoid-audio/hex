//! Separate canonical MIDI constructor qualification. Environmental checks are
//! shared with the pinned std selector, NOT the PCM constructor-family label.
//! Both sized Arc::new and sized make_mut/clone_from_ref_in are audited against
//! this commit; unsupported toolchains/inputs remain Unknown. See the source
//! references and model-ingress audit in docs/native-midi-storage.md.
pub const FAMILY: &str = "canonical-midi-sized-arc-new-make-mut-v1";
pub fn decide(observed: super::pcm_gate::Observation<'_>) -> Result<(), &'static str> {
    super::pcm_gate::decide(observed)
}
#[cfg(test)]
mod tests {
    #[test]
    fn both_model_constructor_paths_require_the_pinned_native_std_observation() {
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
