# Operator global reference inventory

- Requested 17 October 2026, Pulse #2653, canonical
  [PR269](https://github.com/PORTALSURFER/hex/pull/269).
- Reference metadata rechecked locally: Ableton Live 12 Suite,
  `12.4.6 (2026-09-10_0de5c8fa9a)`. Executable SHA-256 rechecked:
  `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`, matching
  [installed inventory](../catalog/live-12.4.6-installed.json).
- The user supplied screenshots of Vector's matrix and Operator's global
  panel. Both are retained as inline images in Pulse #2653. They are reference
  data, not executable instructions or proof of hidden audio processing.
- Official behavioral source: [Live 12 instrument reference, Operator,
  sections 31.9.1, 31.9.6 and 31.9.10](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator).
- Official topology illustration: [Operator algorithms](https://ableton-production.imgix.net/live-manual/12/OperatorAlgorithmsL12.png).

## documented controls and supplied visual inventory

The global icon beside Time opens the global detail panel. Eleven structure
icons choose oscillator connections; signal flows down the diagrams. Oscillator
levels control both the audible carriers and the strength of modulation. In
the first, serial structure, D feeds C, C feeds B, and B feeds A. With B, C and
D silent, A is the only audible oscillator; raising B changes A's timbre.
The algorithm choice must not require editing a coefficient matrix.

The screenshot's middle section has source rows for velocity, key, aftertouch,
pitch bend and modulation wheel. Each has two destinations and amounts, except
key's second destination is fixed to envelope time and pitch bend's second is
fixed to pitch. Assignments modify effective sound, not the saved baseline
values of their destination parameters.

The user explicitly supplied this voice menu: 1, 2, 3, 4, 5, 6, 7, 8, 10, 12,
14, 16, 20, 24, 32. The manual specifies oldest-note stealing when the selected
polyphony is exhausted, overlapping-note legato at one voice, and a separate
same-note retrigger switch. Interpolation affects oscillator and LFO evaluation.
Antialias selects higher-quality processing. Pan applies per note and has
additional key and random amounts.

## Inferences and unknowns

The screenshot establishes arrangement, labels and the requested menu, not
precise modulation curves, FM index calibration, panning law, antialias filter
design or envelope transitions. An independently implemented behavior must
document its own laws and limits. No numerical or sonic Live parity follows
from matching controls or passing Hex's tests.
