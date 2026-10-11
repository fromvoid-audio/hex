# compressor

The registered `hex.compressor` is an original zero-lookahead, stereo-linked
feed-forward processor using the shared Lattice stateful effect contract. Its
version-1 opaque state contains six finite `f32` controls. Generic device
metadata supplies catalog display, editing, persistence, history, and parameter
publication.

The detector follows the larger absolute channel sample. Its envelope uses a
one-pole coefficient prepared from attack time while the input rises and release
time while it falls. The detector level is `20 * log10(envelope)`, with a
documented `-120 db` floor at silence. For delta equal to level minus threshold,
the uncompressed region has zero gain reduction. Above a hard knee, reduction is
`(1 / ratio - 1) * delta`. Within a nonzero knee, the original quadratic curve is
`(1 / ratio - 1) * (delta + knee / 2)^2 / (2 * knee)`. At ratio 1 it reduces no
gain. Makeup is added to the reduction target.

Current gain in dB follows the target through a control-prepared 5 ms one-pole
smoother, then multiplies both channels by the same linear gain. Runtime state is
only the scalar detector envelope and gain value. Parameter revisions can carry
these histories between same-rate compatible instances while keeping the
candidate's new settings and coefficients. Reset restores zero envelope and the
configured makeup gain. Bypass advances the same state while preserving the
input.

The effect declares zero algorithmic latency and a zero-frame finite output
tail: its stored detector and gain can affect future nonzero input, but cannot
produce output on their own after the input becomes zero. `is_quiescent` therefore
describes output silence, not absence of state history. Invalid input frames pass
unchanged without changing detector state so the host can report the fault.
Finite values that overflow during gain multiplication are not silently clipped.

The processor uses an original standard soft-knee law; settings and numeric
responses have not been measured against Ableton Live's Compressor. There is no
lookahead, external sidechain, detector filter, auto mode, RMS path, metering, or
dedicated panel in this slice. It uses the generic shared native-device panel;
native pointer and listening acceptance remain open.

## verification

The device tests cover known knee-law points, a one-sample detector/gain
step-response oracle, stereo linking, ratio-one behavior, malformed metadata,
partition invariance, reset, bypassed state advancement, invalid inputs, and
history copy/retarget compatibility. Engine and storage tests exercise catalog
resolution, ordered track/send/Return rendering, bypass, live parameter changes,
undo/redo, and opaque save/load. The audio callback allocation probe checks
initial and edited revisions, observes routed PCM, and measures admission plus
render for zero total and signed-net allocations. These source tests do not
establish native UI or listening acceptance.
