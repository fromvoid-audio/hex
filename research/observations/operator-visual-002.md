# Operator visual refinement — 11 October 2026

Pulse2332 / PR147. Independent native GPUI composition; no reference assets are
shipped. Source baseline: PR146 merge6c27a05. Hex uses its required lowercase
Ioskeley Mono typography and theme, so exact pixel or font parity is not claimed.

## reference and method

Installed `/Applications/Ableton Live 12 Suite.app` was rechecked as
12.4.6 (2026-09-10_0de5c8fa9a), arm64; executable SHA256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
Read-only screenshot of its existing Operator and the official
[Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)
image informed the comparison. Official image SHA256
`da8e542d1cee761f2faf718ce82be3c9cdc230e4c8d58f6024cc6f71632afac5`.
Private reference captures remain ignored in `research/private/`.

Disposable separately identified Hex app iterations used one new track and an
Operator dropped onto its Sine device. Existing user Hex windows and the Live
project were preserved. Native screenshots at 2x scale showed the reference's
small colored right-edge oscillator badges, label-over-dial placement,
value beside the lower dial edge, dark center, and flat square-marked headings.
The reference gives LFO/filter a selector column and pitch/global only an icon
column. Hex's first iteration still had oversized badges, boxed readouts,
stacked dial values, repeated pitch/global headings and compressed lower rows.
The second iteration corrected the grouping and exposed clipped selector text
and readout units, which prompted fitting the compact frame and readout font.

## resulting supported behavior

The existing 760x184 logical-pixel card retains its three-column layout and
four 40px shell rows. Badge colors are distinct a yellow, b cyan, c blue,
d orange. Badges remain oscillator selection controls; oscillator enable/disable
is absent from the current canonical model. Fixed frequency remains canonical.
Six supported envelope values retain their linear gesture mapping, validation,
Undo and saved units, with ms/s and dB display conversion only. The curve follows
the existing DSP's linear attack and exponential decay/release with 1e-5 cutoff,
including exact endpoints. No DSP, schema or audio lifecycle changes were made.

Reference-only richer envelope controls and simultaneous oscillator/envelope
subsections remain outside this supported layout. LFO/filter/pitch/global and
fine controls remain view state awaiting Pulse2325–2328. Selection tabs retain
Hex's three supported pages. Diamond handles and grid differ from Live. Visual
comparison is evidence of placement, not functional/listening acceptance.

## responsiveness evidence and limits

The input trace updates pending envelope values and gesture snapshots immediately
on pointer movement; visual readout does not wait for audio acknowledgement.
The user clarified that the curve follows the pointer slowly. The drag handler
formerly deferred display geometry/readout derivation to the whole editor render.
It now updates retained graph and numeric entities directly in the input turn,
avoiding a whole-editor notification for each successful Change. Begin, terminal,
cancel and error paths retain their redraw and canonical gesture semantics.
A focused regression checks the node/readout before another full editor layout
or worker pump, while canonical state remains unchanged. Disabling the direct
update reproduces delayed node feedback; the repaired path passes.

The root service poll remains 16ms and preview preparation remains a bounded
worker protocol. Those contracts were preserved. Curve generation formerly ran
on each root redraw; now unchanged values/window/selection/style bypass model
construction and validation, while changing values use fixed sampled factors.

A standalone optimized Rust1.98.1 measurement on Apple M5 Pro extracted the actual
model drawing functions: 10,000 warmup iterations then 1,000,000 timed iterations,
`Instant`, `black_box`, one run. Rebuilding a smooth model took132.65ns/iteration;
an unchanged-value cache comparison took2.17ns. This is a narrow headless
microbenchmark, with other app/OS work uncontrolled, not before/after native
pointer-to-paint latency or proof of perceptible improvement. Raw program remains
ignored at `artifacts/operator-curve-profile/`. Native drag/audio latency and
percentiles still require user acceptance; do not call the editor lightning fast
based on these numbers.

## validation

Focused native GPUI source tests cover badge/row geometry, page placement, dB
keyboard edits and envelope drag/cancel/Undo/persistence. The curve regression
checks exponential stage midpoints, exact endpoints and zero/max time bounds.
Final full local CI and native screenshot result are recorded in PR147 and Pulse.
Manual checklist: compare all four rows at normal size; inspect fixed and ratio
modes; drag each envelope node repeatedly, cancel with escape, release outside,
Undo/Redo and save/reload; type ms/s/dB values; switch oscillator/pages/tracks;
confirm visual response and audio preview separately. Pulse2330 remains the
user's native interaction/listening gate.
