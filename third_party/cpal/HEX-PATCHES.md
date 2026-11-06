# cpal 0.17.3 Core Audio route probe and bounded output

Baseline: the published crates.io `cpal` 0.17.3 archive, SHA-256
`d8942da362c0f0d895d7cac616263f2f9424edc5687364dfd1d25ef7eba506d7`,
with upstream VCS revision `fd3b945bffcaa493fa7cb5ceddf9db1f9330fd30`.
The published archive and Apache-2.0 license are retained. The only source
change is `src/host/coreaudio/macos/device.rs`.

The macOS default-output-configuration query now reads the device buffer-frame range
directly from the Core Audio device property instead of constructing a temporary
Audio Unit. Device UID, default stream format, sample rate, range and error
reporting remain part of the same CPAL contract. Unsupported or malformed
property results still return an error.

The macOS-only bounded output entry point pins the negotiated device to an
AUHAL stream, sets the hardware buffer and asks the initialized Audio Unit
for its client-frame maximum. It refuses before stream start if the configured
rate or buffer no longer matches, or the maximum exceeds the caller's ceiling.
The reported hardware-buffer duration uses the device's rate. Failed startup
attempts restore the prior buffer if it still equals the requested value.
The generic output and input entry points keep their existing setup.
The direct device-property approach also appears in upstream CPAL 0.18.0,
archive SHA-256 `d9dd2b2151ebb4d5866c804d89fe28244bfb6b74481b9b4d406e4ec4d7f88ce5`,
VCS revision `45fe976149d5ea0c01b30546e26b3c867285c539`.

This excluded dependency preserves the existing CPAL/Core Audio native boundary;
Hex workspace code does not add unsafe code or a second device owner. The test-only
counter and native regression exercise repeated default-output queries, and compare
the direct range against the former Audio Unit query on the selected device:

```sh
cargo test --manifest-path third_party/cpal/Cargo.toml --lib --locked route_probe_regression
```

The test requires a working macOS default output. It tests the configuration
query, not device deadlines or audible output. The isolated vendored manifest uses
its published lockfile; workspace consumers use the root `Cargo.lock`.
`hex-patches.diff` records the source delta. Reassess this backport when upgrading
CPAL; do not edit the Cargo registry cache. Run `python3 scripts/check-cpal-patch.py`
to verify the complete extracted archive against `hex-provenance.json`.
