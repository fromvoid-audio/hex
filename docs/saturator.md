# Saturator

The registered `hex.saturator` is an original, stateless nonlinear stereo
processor using the existing Lattice native-device contract. It can be ordered
with other effects on track and Return chains. Its version-1 opaque state stores
three finite `f32` controls and one curve index; generic device metadata handles
catalog display, parameter editing, persistence, undo, and parameter publication.

Drive and output are authored in dB. Drive multiplies each sample by
`10^(drive_db / 20)` before shaping. The default **soft clip** curve is linear
through ±0.5 and then follows `sign(x) * (1 - 0.25 / abs(x))`; this joins with
unit slope and approaches ±1 as magnitude increases. **Digital clip** clamps the
driven value to [-1, 1]. Output multiplies the shaped signal by
`10^(output_db / 20)`. Dry/wet is a convex blend of that wet signal with the
untouched input; at zero wet, the runtime leaves every input bit unchanged.

Validated ranges are drive [-36, 36] dB, output [-36, 6] dB, dry/wet [0, 1],
and the two named curve choices. Settings and gain coefficients are prepared on
the control side. Each admitted runtime owns only fixed scalar state, declares
zero algorithmic latency and a zero-frame finite tail, and reports quiescent.
Processing performs no allocation and preserves nonfinite input so the audio
fault boundary can report it rather than masking it. A nonlinear device refuses
matrix-only preparation; its full-chain matrix contribution is identity and its
processor is supplied through the existing stateful device seam.

These shaping laws are original choices for Hex, not measured reconstructions of
Ableton Live's Saturator curves. There is no oversampling, anti-alias filter,
coloration, or Live transfer-law claim in this slice. Native listening and
visual acceptance remain open.

## Compact panel and transfer preview — Pulse 2312

The dedicated 230-pixel-wide panel uses the shared rounded compact frame,
selector, editable dials and bounded transfer plot. The curve chooser sits above
the plot; drive, output and dry/wet sit below. Dry/wet uses the shared percent
formatter/parser. Both track and Return devices use the same composition at the
existing fixed rail height. No unimplemented Live controls are drawn.

The preview samples the actual device processing function at 129 points over
input amplitudes [-1, 1]. Its symmetric output axis is ±max(1, output gain),
so positive gain expands the range instead of clipping the pictured output.
The ranges are shown beneath the plot. This is a static transfer curve, not a
live signal meter or an anti-alias quality guarantee. Device/engine owns its
sample law; the widget draws normalized points without DSP knowledge.

Preview data is cached by canonical state and effective local parameter values.
An in-progress gesture or pending control edit updates it before worker
acknowledgement. Unchanged frames reuse the data; owner/project changes and
device removal retire it with other panel state. The public Live manual image
informs composition only; no reference image or proprietary assets are shipped.

## Verification

The owning crate tests assert known soft-clip anchors and symmetry at the join,
cover large finite extrema, verify digital clipping and the audible
drive/output/wet controls, preserve nonfinite values, and exercise generic state
validation and the required nonlinear preparation refusal. Track/Return project
render, parameter-publication, and callback allocation probes are covered by the
integration validation recorded with the Saturator PR; source tests alone do not
establish native UI or listening acceptance.
