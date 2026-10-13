# Output sample-peak presentation

`hex-ui::output_meter` is an isolated presentation component. It is not wired
into the canonical editor status bar yet: that existing producer consumer still
clamps amplitudes to 0..1. Root integration must be coordinated with musical
keyboard routing work; controller and native telemetry changes are separate.

The future adapter must pass raw `Status::peak_left` and `peak_right` values,
without an earlier clamp. Those fields report maximum absolute stereo samples
across the last CPAL callback before output-format conversion. They are separate
relaxed atomics, not a coherent stereo snapshot, and carry no timestamp. Mono
output subsequently averages the channels. This component does not measure
hardware clipping, true peak, RMS, or time-integrated peaks.

`MeterInput::Last` produces a visible `last l … r … dbfs` readout and independently
maps each channel. Zero, including negative zero, reads `-inf`; positive values
below amplitude 0.001 read `<-60`. The bar's bottom is -60 dBFS, its top is 0 dBFS,
and interior levels use `20 log10(amplitude)`. Values above unity retain a positive
dB readout and `!`, with an instantaneous notched top indicator. Above-unity
classification uses the raw amplitude before decimal rounding. Large positive
readouts are explicitly bounded at `>99.9!`. Negative/nonfinite inputs read
`invalid` with empty neutral gauges. A valid opposite channel remains readable.

Offline, stopped and fault inputs have distinct text and neutral gauges. The
consumer selects these availability states; the component has no telemetry
ownership, clock or retained peak state. A subsequent below-unity sample removes
the indication immediately. There is no hold, release decay, ballistics, stale
expiry, or claim that unchanged samples are fresh. Descriptive text explicitly
states that age is unknown and the channels are independently reported. Reliable
callback expiry would require a separate owner-approved telemetry contract.

The slot is fixed at 248×22 px, with two 6×16 px bottom-anchored gauges. Readout
text uses the root's bundled Ioskeley font, lowercase `theme::text` and a derived
foreground meeting the existing text contrast policy on the actual surface.
Saved theme colors are unchanged. Numeric text and `!` convey states without
relying on color. The component supplies descriptive textual state for future
consumers; it does not register AppKit accessibility names or roles.

Tests exercise finite/floor/zero/negative/NaN/infinite/above-unity boundaries,
independent channels, rounding seams and bounded text. Actual element styles are
checked under the default and an existing valid low-contrast theme. A rendered
960×600 status-budget fixture preserves six child slots and five 8 px gaps,
leaving 154 px for its flexible message, with fixed 248×22 meter geometry. It
checks scale-aware pixel rounding, bottom anchoring, all availability states,
instantaneous indication removal and resize without moving fixed neighbors.
This fixture does not wire or validate the live editor's telemetry consumer.

GPUI's Linux TestPlatform uses NoopTextSystem and no-op native draw. These tests
do not prove real Ioskeley glyph fit, native contrast perception, GPU presentation,
physical input or native meter readability. Optimized macOS acceptance must check
960×600 glyph fit and truncation, default/custom themes, asymmetric channels,
silence/below-floor/unity/above-unity, stop/pause/recovery/fault and actual output
format/channel behavior. OS accessibility remains work at the real AppKit bridge.
