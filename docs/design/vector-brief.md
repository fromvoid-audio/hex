# Vector device brief

Vector supports deep four-operator FM sound design through a clean, direct workflow. It inherits Hex’s purpose and conventions from the [project overview](../../README.md), uses the [shared device host](device-host-brief.md), and follows the local Codex product design philosophy. Its musical depth is Operator-inspired; its visuals and general modulation belong to Hex.

## Working surface

Every oscillator exposes ratio, fine Hz offset, one level control, waveform choice, and its own keytrack slider. Fixed routing presets, feedback, filter cutoff/resonance, mono/poly and glide remain available while editing envelopes. Custom routing is outside the first iteration. Phase/retrigger live in oscillator options; voice count lives in device settings. General modulation uses host modulators.

## Envelope editing

Selection is explicit. Level/tuning edits never select another envelope. One oscillator envelope is editable, with the other three ghosted on the same locked nonlinear time scale. Filter selection replaces this with its one independent envelope. ADSR values and curve shaping remain directly accessible. Initial and peak levels are graph points whose exact values appear when selected.

All four oscillator envelopes share these ranges: attack 0 ms–20 s; decay 1 ms–60 s; sustain −∞–0 dB; release 1 ms–60 s. These are user-verified displayed Operator ranges, not claims of identical DSP behavior. Filter level units/ranges remain a separate open decision.

Attack and decay loop; release remains outside the loop. Free looping follows authored timing. Tempo sync fits the loop to a beat division. Note-pitch sync fits it to the played-note period, with a ratio such as 0.5×, 1× or 2×. Both synced modes preserve relative stage timing and curves. Timing controls appear only with looping enabled. Note-off exits immediately into the normal release time from the current level. Whether loop boundaries reset to the initial level or restart from the current level remains undecided.

## Waveform editing

The curve/harmonics editor expands immediately to the right of the oscillators. Changes audition live; there is no Apply/Cancel. Closing the panel keeps edits; Undo restores unwanted changes. Native freehand editing, audio preview, and gesture-level Undo remain implementation requirements. The Frame document provides illustrative edited states, not synthesis.

## Visual and keyboard direction

Keep a 200 px device height and expand horizontally. Use neutral graphite surfaces, blue selection, restrained oscillator colors and readable lowercase Ioskeley Mono text. Reclaim space by moving occasional controls, rather than shrinking key values. Tab exposes controls in a logical order; selection and opening panels do not commit sound edits. Keyboard value editing preserves normal text entry. Precise graph keyboard gestures remain to be defined.
