# original Operator LFO — Pulse2325 / PR154

The primary approved this original contract on11October2026. The public Live12
Operator manual informs the control inventory; numerical parity with installed
Live12.4.6 is unknown. Native UI, listening and reference acceptance remain open.
This integration uses generic descriptors, opaque versioned state, canonical
project commands and shared UI controls. Prototype local values are not imported.

## Stable state and modulation

State version6 appends44 bytes after the exact320-byte version5 prefix, for364
bytes and88 editable descriptors. Eleven little-endian four-byte words store:

| ID | control | domain / default |
| --- | --- | --- |
|82|enabled|boolean / false|
|83|waveform|sine, triangle, rising saw, square, sample-and-hold / sine|
|84|sync|boolean / false|
|85|free rate|finite f32,0.01–100Hz /1Hz|
|86|division|1/16,1/8,1/4,1/2,1,2,4,8,16 quarter-note beats per cycle /1|
|87|amount|finite f32,0–100% /0|
|88|retrigger|boolean / true|
|89–92|pitch destinations a–d|boolean / all true|

Booleans require integer0/1; choices require their exact index domains. All
inactive fields are validated. Unknown versions, wrong lengths, invalid floats,
booleans or choices are refused atomically. Versions1–5 preserve their validated
prefix bit-for-bit and append disabled defaults. Save/reopen and undo/redo use
the existing generic storage/history path; no project schema branch is added.

For each selected oscillator, frequency multiplies by
`2^(wave * amount / 100)`: maximum100% gives±12 semitones, including fixed tuning.
Unselected oscillators retain their base tuning. Filter destinations belong to
2326; pitch envelope and global controls remain2327/2328. Disabled LFO retains
the legacy analytic/feedback path. Enabled LFO, including amount0, selects the
new integrated-phase mutable law and coexists with effective self-feedback.

Periodic waves lie in[-1,1]. Sine/triangle begin at0; triangle has+1 at1/4 and-1
at3/4. Saw begins-1 and wraps at1; square is+1 before1/2 and-1 thereafter.
Sample-and-hold uses the SplitMix64 finalizer of the wrapping cycle count plus
`0x9e3779b97f4a7c15`, mapping its upper53 bits to[-1,1). It changes only on a cycle
boundary. There is no shared RNG. Fractional phase is f64 modulo1; cycle count
wraps u64. Carrier phases integrate quantized u64 phase steps, wrapping normally.

## Clock, events and discontinuities

Free phase advances rate/sample-rate per performance frame. Sync retains a
separate authored division and advances by consecutive Q32.32 effective
performance-beat differences divided by beats-per-cycle. The borrowed
`NativeInstrumentClock` seam reads the engine's prepared tempo timeline, including
an inclusive segment-end lookup. It neither reconstructs project maps nor uses a
120BPM fallback. Clock values must be deterministic and nondecreasing; a missing,
invalid or backwards sync clock refuses the batch before PCM/history mutation.
Clock validation and sampling are bounded per-frame lookups, shared across voices.

NoteOn/NoteOff apply before their exact-offset sample, in supplied order; the
exclusive block end belongs to the next block. Retrigger starts each occurrence
at phase0. Without retrigger, notes use the device phase, which advances during
silence. NoteOff keeps carrier, LFO and feedback history through release. Pause
freezes the performance clock. Fresh playback/reset clears voices, phases and
feedback; sample-and-hold restarts deterministically. Host loops, launches and
source boundaries retain established occurrence ordering and performance clocks.

Direct SessionPlayback seek refuses when mutable history requires reconstruction.
The Session controller's bounded worker preroll reconstructs free/sync LFO and
feedback history for accepted seeks. Export uses the same prepared renderer on
linear Arrangement time. Stop retains existing host ownership;2305 is neither
repaired nor declared reproduced here.

## Live transfer and finite admission

Exact copies preserve voices, gates, carrier/LFO phases and feedback. Enabled-LFO
targets can retarget any LFO field when the entire320-byte base state, sample rate
and table identity match. A control-prepared proof binds the target configuration;
the callback transfers fixed history at its current boundary. Rate, waveform,
amount, destinations and retrigger changes take effect immediately without a
smoothing ramp. Both per-note and device phases are maintained so retrigger edits
have defined existing history. Base edits and analytic/feedback/LFO processing-class
transitions retain the host's quiescence/refusal policy rather than losing voices.

Admission conservatively requires every oscillator's base frequency below Fs/4,
regardless of depth/destination, so the complete one-octave excursion stays below
Nyquist. Non-sine harmonic caps use twice the base step. Authored Arrangement and
Session notes are validated before admission; runtime event batches repeat the
check. Existing16-voice,256-event/processor and memory ceilings remain unchanged.
The LFO twin is3864 bytes, pair7728 against8192; the disabled feedback pair remains
8064. Runtime/proof/rollback state is fixed; no callback heap buffers, locks, I/O,
table initialization or unsafe workspace exception are added.

## Verification and remaining gates

Original sine-output integration oracles cover exact event offsets, release,
tempo discontinuities and retrigger; waveform landmarks and deterministic hold,
partition independence, history transfer, reset and invalid state/event/clock
refusal have focused tests. Arrangement/Session comparisons, prepared PCM16
export, worker seek, loops, canonical history and persisted state are exercised.
Callback probes measure total and signed-net allocation counts/bytes for active
free/sync LFO with feedback, release, shutdown and repeated amount transfers.

Release profiling uses `hex-engine/examples/profile_operator_lfo.rs`,16 held
voices at48kHz,128-frame blocks,100 warmups and2000 measured blocks, Instant and
black_box. A268800-frame partition oracle and PCM hash run outside timing.
Before/after disabled and feedback modes use the original task baseline; active
free/sync modes are measured separately. These Linux VM timings do not establish
native audio deadlines or Live performance parity.

Source completion requires full localCI on the exact clean final commit. Linux
UI/workspace compilation currently encounters the unchanged locked external
xattr0.2.3/libc0.2.190 ENOATTR incompatibility (Pulse2349); maintenance2348 owns
native prerequisites and reusable environment publication. Partial portable tests
cannot satisfy that gate. PR154 remains draft and unmerged while required checks
and native UI/listening/reference acceptance remain open.

Measured11October2026 on a KVM Linux x86_64 VM reporting Xeon Platinum8370C,
5 virtual CPUs, Rust1.94.0, release default profile. Median of three runs (us per
128-frame block); no compiler running during the retained final series:

| path | before p50 / p95 / p99 | after p50 / p95 / p99 |
| --- | --- | --- |
|disabled analytic|134.659 /306.682 /614.883|135.748 /301.779 /501.238|
|disabled with feedback|91.528 /169.242 /322.817|91.504 /172.462 /319.317|
|enabled free|—|165.867 /322.611 /650.797|
|enabled sync|—|174.961 /358.404 /655.075|

Disabled analytic median changes+0.81%; feedback-0.03%. VM scheduling noise
precludes strong regression/deadline conclusions. Disabled PCM hashes match the
baseline exactly: analytic `c1424109536d1e1b`, feedback `ff054452c66eed63`.
Active hashes: free `768a526ab4de6e95`, sync `3712dc640b2a3dcb`. Retained raw series:
`artifacts/lfo-profile-final.log`; baseline task source `aa63b849`, same standalone
profiling harness in a detached worktree with a separate release target. The
harness and final DSP source are committed together; timing excludes preparation,
state/history editing, oracles and publication.
