# microphone privacy trace — pulse2750 / pr360

## observed evidence and source identity

The ticket records a TCC refusal at 13:53:27.995 local time on 17 October 2026
for PID 38152 / `org.portalsurfer.hex.dev`: a microphone authorization request
without `NSMicrophoneUsageDescription`. Its reported bundle source was
`2530036399889929bca64d69f78204ee85b2a88a`. This is point-in-time log/bundle
evidence, not a call stack, proof of an implemented recording feature, or proof
that every later executable requests microphone access. Bundle metadata alone
does not establish the code loaded into a running process.

This audit inspected published current main
`5bd4220d08f37f422eb17561bc4a350620e0c4ef`, tree
`cabfdd2ed5b11c4307d9c088f5fbfd2d844a2e45`, and the named historical source.
Both use pinned CPAL 0.17.3 (Cargo.lock checksum
`d8942da362c0f0d895d7cac616263f2f9424edc5687364dfd1d25ef7eba506d7`)
and coreaudio-rs 0.14.2. No native backend, input stream, permission request or
installed application was executed during this source inspection.

## actual app and dependency path

1. `crates/hex-desktop/src/main.rs` enters `native_audio::backend()` for the
   project editor; `--demo` uses `hex_audio::AudioOutput::open()` instead.
2. The editor backend installs `NativeAudioSink` on the existing Session worker.
   Its `poll_open` uses `hex_audio::request_worker_open`; the existing lifecycle
   owner reaches `WorkerPreparedOutput::open_with_lease` and `device::open_prepared`.
3. Both project and demo output paths call `default_output_device`,
   `default_output_config` and `build_output_stream`. They do not enumerate
   input devices, open an input stream, or call CPAL `Device::description`.
   The labels use the selected output rate/channel count, not that description.
4. CPAL's macOS `default_output_device` reads the default-output property.
   `default_output_config` calls `default_config` with the output property scope.
   That path and `build_output_stream_raw` call `audio_unit_from_device(false)`.
   A current default output uses `IOType::DefaultOutput`; another selected output
   can use `IOType::HalOutput`. `AudioUnit::new` initializes the Apple unit.
   CPAL's explicit input-enable branch is conditional on `input == true`.
   `Scope::Input` on an output unit's render bus means feeding output samples;
   it does not by itself mean physical microphone capture.
5. A distinct CPAL `Device::description` implementation calls
   `supported_input_configs` and `supported_output_configs`. Its input-config
   probe can construct an input AudioUnit. No call to this description API was
   found in the app output path above. Do not introduce that probe merely to
   display an output-device label.

Searching app crates and GPUI source for microphone usage keys, input-device/
stream calls and AVCapture audio authorization calls yielded no app capture
entry point, in either named Hex source. These negative source searches cannot
prove the behavior of macOS, a device driver, an external library, a changed
default route, or a different process image. The old TCC request's concrete
caller remains unattributed; no "old input probe fixed" claim is established.

## declaration and refusal boundary

`scripts/hex-test-build.py` assembles the repository-owned Info.plist. Its current
profile has no microphone purpose key. `docs/features.md` still marks audio
recording as planned; gain-automation touch recording, MIDI controllers, file
import, output spectrum capture and offline rendering are not microphone input.
Adding a purpose string that claims recording/monitoring now would describe an
unsupported feature and could turn an unrelated authorization attempt into a
prompt. No such declaration, input feature, new framework or dependency patch is
introduced by this audit. Any future supported input path must have an accurate
lowercase purpose declaration and explicit user-intent/denial tests before its
first authorization/capture request. That condition is not a claim that input
support exists now.

Native output setup errors already travel through `EditorAudioEvent::Unavailable`
with generation/restart identity. The editor retains canonical project state;
recovery failures show `audio unavailable · editing and saving remain available`
and the underlying error. This does not classify every native error as a
microphone denial or assert that recording works. An existing real Session-worker
regression, `native_audio::tests::failed_output_setup_keeps_canonical_editing_and_saving_available`,
injects output-open failure and verifies a later canonical edit, journal and
create-new save. It passed in the unchanged-source BPM full run
`20261018T131225Z-54ebcb9c`, step 13 `audio-allocation-probe`; original summary SHA256
`83457a8c09c7b7a4239443e1f7f7693fb1c862b36c89de1e62c381d2eb6fe8f9`.
That run checked candidate 3219f059, identical tree to actual 5bd; it is reused
historical evidence for that implementation, not relabeled CI for this document
or native microphone authorization evidence. Its original bytes are in
`evidence/hex-tempo-2720-3219f059-v1` / commit 81bc20e21e4c81807a9ec5ba341d08ea49ccb7ad.

## finite coordinator-owned attribution request

There is no source-qualified microphone fix to deliver yet. PR 360 stays draft;
actual trigger/refusal attribution remains open. No broad permission grant,
purpose-string guess or native lifecycle redesign is justified by this trace.

1. In the coordinator's sole Mac lane, first inspect already retained TCC logs
   around 17 October 2026 13:53:20–13:53:35 local time and PID 38152. Correlate
   request/responsible/audit identities, the exact executable identity if it was
   retained, service and any existing caller stack. Inspect only the relevant
   saved records; do not dump a privacy database or unrelated user paths. If the
   records are unavailable or lack a caller, return that limit explicitly.
   This read-only phase needs no app launch, save/quit, backend activation or
   user interaction. No TCC reset, grants, permission API, debugger attachment,
   new library load, keychain operation or infrastructure change is requested.
2. Source inspection of the currently delivered/scheduled executable can verify
   its existing Info.plist and sealed receipt/executable source identity without
   changing it. Absence/presence of a purpose key does not prove whether a
   microphone request occurs. Preserve the original historical log separately.
3. If existing records cannot identify the caller, actual startup/route behavior
   requires a separately scheduled **user-present** observation, after the
   coordinator chooses an immutable optimized source/build. Do not launch Hex
   or instantiate a backend while the user is away to manufacture this evidence.
   Before any launch, normal user save/quit is required if another app instance
   is open; never force-quit. Capture a bounded startup log window and exact
   build/signature/route identity. Do not start capture or request input. If an
   unexpected permission dialog appears, stop at it for the user's decision;
   do not answer, grant, reset or retry automatically. Retain refusal/error and
   verify the project remains editable/saveable without claiming recording.
4. Return separate results for source trace, historical OS attribution, current
   no-input startup behavior and any future intentional input permission/capture.
   Unperformed items remain unrun. A route-dependent HAL initialization finding
   may require a bounded dependency/lifecycle contract with the native audio
   owner before edits; no compiler/resource/custody/registry work is authorized
   by this trace. Do not request a general native suite or repeat MTS checks.

Installed application changes and optimized delivery remain coordinator-owned.
This audit does not change the installed 7fe/build 2145711247 app, microphone
permissions, project files, schema, signing, recording or sampler support.
