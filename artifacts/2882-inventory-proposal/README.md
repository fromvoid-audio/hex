# proposed singleton Main runtime binding (unapplied)

Canonical session_playback.rs beforeSHA256 b1e5ea3052924061a757aa43c4272dd34fa5787e9c818d0eff996ef5cd9232b3.
Canonical lower session_playback/candidate_storage.rs beforeSHA256 c8530d78fc7a2bc9ff01cf7324f4e99761850e5e44286122f64cf39e33c0460f.

Correction: the lower candidate_storage file EXISTS on canonical base (6643B); initial lookup mistakenly checked engine/src/candidate_storage.rs. Neither this base hash nor historical Root overlay hash asserts current Cloud5 dirty-byte equality.

The literal lower-hook patch adds the actual Main chain to existing exact storage traversal, NOT a placeholder. Existing function is intentionally a narrow fresh Utility-class constructor; unsupported provider storage stays fail-closed under that constructor's existing contract. Future395 broader canonical+physical owner inventory must bind its actual Main typed fields as well, on current owned bytes with Cloud5/Root coordination. No unseen bytes copied or edited.

Main prepared field: Project.main_device_chain: Arc<[hex_lattice::PreparedRuntimeInstance]>. Existing-typed constructor accessor: pub(crate) fn new_main_device_runtime(&self) -> Result<hex_lattice::NativeRuntimeChain, hex_lattice_contract::DeviceContractError>, reserved Vec of actual prepared instances then NativeRuntimeChain::new_prepared. Runtime field Main is concrete NativeRuntimeChain. Existing physical owner kind0..8 stay unchanged, requested singleton Main kind9(index0). Additive ProcessorChainOwner::Main unit variant carries actual stable DeviceId without synthetic TrackId/ReturnId.
