# cpal 0.17.3 Core Audio route probe and bounded output

Baseline: the published crates.io `cpal` 0.17.3 archive, SHA-256
`d8942da362c0f0d895d7cac616263f2f9424edc5687364dfd1d25ef7eba506d7`,
with upstream VCS revision `fd3b945bffcaa493fa7cb5ceddf9db1f9330fd30`.
The published archive and Apache-2.0 license are retained. Source changes are
`src/host/coreaudio/macos/device.rs` and its three public re-export locations
(`macos/mod.rs`, `coreaudio/mod.rs`, `platform/mod.rs`).

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
The generic output entry point keeps its existing setup. The input entry point
forwards device-disconnect errors for every selected input, including the
system default, to the supplied error callback. The selected input is pinned;
a change of system default does not silently transfer its capture to another
device. As elsewhere in this backend, a contended error callback may be skipped.
The direct device-property approach also appears in upstream CPAL 0.18.0,
archive SHA-256 `d9dd2b2151ebb4d5866c804d89fe28244bfb6b74481b9b4d406e4ec4d7f88ce5`,
VCS revision `45fe976149d5ea0c01b30546e26b3c867285c539`.

This excluded dependency preserves the existing CPAL/Core Audio native boundary;
Hex workspace code does not add unsafe code or a second device owner. The test-only
counter and native regression exercise repeated default-output queries, and compare
the direct range against the former Audio Unit query on the selected device:

```sh
cargo test --manifest-path third_party/cpal/Cargo.toml --lib --locked route_probe_regression
cargo test --manifest-path third_party/cpal/Cargo.toml --lib --locked input_disconnect_regression
```

The route test requires a working macOS default output. The disconnect test
exercises the shared input callback forwarding without a physical device; it
does not prove native unplug behavior. Neither proves device deadlines or audible
output. The isolated vendored manifest uses
its published lockfile; workspace consumers use the root `Cargo.lock`.
`hex-patches.diff` records the source delta. Reassess this backport when upgrading
CPAL; do not edit the Cargo registry cache. Run `python3 scripts/check-cpal-patch.py`
to verify the complete extracted archive against `hex-provenance.json`.

## macOS callback host timebase

Both macOS stream constructors acquire and validate the Mach timebase before
configuring the device or registering callbacks. Apple caches the ratio after
its first successful native lookup. Initialization can invoke a kernel trap.
Each callback now converts Core Audio's supplied host ticks and its existing
device-buffer offset with the retained ratio and widened integer arithmetic.
Malformed ratios fail stream setup. Pre-boot or unrepresentable timestamp
pairs return an Audio Unit callback error before delivering data to CPAL's
caller. The conversion path does not query the OS, lock or allocate.

The supplied timestamp identifies the device callback's time domain. The
conversion measures neither later VST queue publication nor elapsed host
callback work. The separate Hex CPU meter keeps its existing clock. iOS
retains its original timestamp path. The focused `host_timebase_regression`
tests use the native ratio and independent normal, overflow and invalid-input
oracles. Physical device timestamp accuracy remains a separate check.
See [Apple's timebase implementation](https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.1.9/libsyscall/wrappers/mach_timebase_info.c).

## Control-side declared output timing

The safe macOS-only `Stream::output_timing_on_control` query uses the actual
AudioUnit and its pinned device. `platform::CoreAudioOutputTiming` reports the
buffer, safety offset, device latency, uniform output-stream latency (hardware
frames), device/client rates, and the separate AudioUnit latency (seconds).
It locks and calls native properties on control, never on an audio callback.
Enumeration is capped at 16 streams. Two equal snapshots detect observed changes;
missing, malformed, nonuniform or changed observations refuse. This bounds query
work, not OS call latency, and does not make multiple property reads atomic.

The checked `hardware_path_client_frames` conversion includes only the declared
buffer/safety/device/stream path and rounds up once with exact integer arithmetic.
Fractional nominal rates refuse instead of introducing floating-point rounding. It deliberately excludes the
AudioUnit value: its overlap with device latency is not established by this query.
Do not add that value blindly or treat the record as complete physical latency.
The query grants no ongoing route validity. A future source-bound live presentation
consumer must establish composition and invalidate on relevant rate, buffer,
stream, safety and latency changes before reporting known timing. No current
playback, callback timestamp or VST presentation behavior changes here.

This extends the existing excluded CPAL/Core Audio property boundary; no Hex
unsafe code, new device owner, callback native query, scheduling or UI shell is
introduced. New FFI reads remain inside the dependency with checked sizes and
initialized fixed storage. Existing publication and lifecycle owners remain.

```sh
cargo test --manifest-path third_party/cpal/Cargo.toml --lib --locked -- output_timing_regression route_probe_regression input_disconnect_regression --nocapture --test-threads=1
```

The native probe constructs a pinned, unstarted output Stream and reads its real
properties without changing hardware buffer/rate or rendering audio. It verifies
the public typed control API, not audible or physical latency. Pure cases cover
unit conversion, overflow, invalid inventory, changed snapshots, unavailable
properties and nonuniform streams. Research provenance is recorded in
`research/catalog/coreaudio-output-timing-2026-11-17.json` in the Hex repository.

The optional `watch_output_timing_on_control` API binds a receipt to the
existing Stream. Device, default route, stream and AudioUnit listeners latch
invalidation for the original observation. Control also rereads the original
properties before returning timing. A new watch replaces the previous one;
play/pause transitions and stream destruction retire the watch. Notifications
can arrive after the property changes, so revalidation describes the checked
instant. The eventual live caller must handle invalidation before establishing
a persistent presentation plan.

Each of sixteen process slots supports one native watch. Listener userdata is
an opaque serial and slot index, never a Rust pointer to storage. Notifications
use one matching atomic compare-exchange with no lock, allocation or property
read. Retirement invalidates before unregistering. A failed unregister
quarantines its already-invalid slot. Late callbacks cannot affect a newer
receipt or access a disposed Stream. The native regression registers on an
actual unstarted output and injects a delivered signal through the registered
shim. Hardware property-notification delivery and physical timing still need
native acceptance.
