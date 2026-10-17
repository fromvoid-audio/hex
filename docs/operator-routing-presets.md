# native Operator routing presets

Pulse2187 adds one generic choice descriptor, `routing preset`, with stable
parameter ID61 and labels `custom`, `serial`, `parallel` and `two pairs`. These
are original Hex routing topologies for the existing sixteen source-major
modulation depths and four carrier weights. They do not copy a Live algorithm
catalog or claim numerical or preset parity.

The named layouts are:

- `serial`: op4 → op3 → op2 → op1, with each modulation depth `0.25` and op1
  carrier weight `1.0`;
- `parallel`: no modulation edges and all four carrier weights `0.25`;
- `two pairs`: op2 → op1 and op4 → op3, each depth `0.25`, with op1 and op3
  carrier weights `0.5`.

Selecting a named layout atomically replaces only the twenty stored routing
values. Oscillator settings, envelopes and waveform choices remain unchanged.
New Vector instances use the `serial` topology, with A at level `1.0` and B/C/D
at level `0.0`. Raising B modulates A without a matrix edit. Existing saved
levels and routes are decoded unchanged.

The selector is derived from exact numeric equality of all twenty routing
values; it stores no extra marker and leaves the state version and payload size
unchanged. Selecting `custom` is a no-op on routing, not a persistent mode. A
manual topology edit reads back as `custom` unless the resulting matrix and
carrier weights exactly equal one of the named layouts. Editing oscillator,
envelope or waveform values does not affect the topology readback. A later
named selection replaces the current custom topology.

Tests cover exact layouts, serial defaults, descriptor and facade readback,
atomic field preservation, idempotence, invalid choices/types, one-bit-near
classification, matrix and carrier edits, custom no-op behavior and transitions
from manual routing to another named layout. A canonical Session integration test verifies one preset edit restores the exact
original state with one Undo and the selected state with one Redo; no preset-specific storage or DSP path is
introduced.

## Vector algorithm selection

The global panel presents eleven clickable four-operator topology diagrams.
The single shared catalog lives in `operator_state/algorithms.rs`; `hex-engine`
exports it to the UI so the diagram edges and prepared routing cannot diverge.
The normal panel uses oscillator levels as the modulation/carrier amounts. The
global page never exposes the old matrix. Custom legacy routing preserves
authored negative routes and carrier weights and shows a `custom` readback
marker until a diagram is selected. Selecting a diagram replaces exactly the
existing twenty routing fields in one ordinary canonical parameter edit with
one Undo/Redo entry. Levels, enable flags, envelopes, tuning and waveform banks
are preserved.

Stable derived parameter ID600 has `custom` at choice0 and algorithms1–11 at
choices1–11. Selecting custom is a no-op; exact matching of all twenty routing
fields determines readback. No algorithm marker or state suffix is stored.
Legacy ID61 choices retain their original edge depths and normalized carrier
weights. Their topologies correspond to algorithms1,8 and11, but only serial
shares exact numeric readback: new algorithms use unit carrier weights so an
a-only patch keeps its gain when additional silent carriers are selected.

| Algorithm | Modulation edges | Audible carriers |
| --- | --- | --- |
| 1 | d → c → b → a | a |
| 2 | c,d → b → a | a |
| 3 | c → b → a; d → a | a |
| 4 | d → b,c; b,c → a | a |
| 5 | d → c; c → a,b | a,b |
| 6 | d → c → b | a,b |
| 7 | b,c,d → a | a |
| 8 | b → a; d → c | a,c |
| 9 | d → a,b,c | a,b,c |
| 10 | d → c | a,b,c |
| 11 | none | a,b,c,d |

The inspected reference is the official [Live12 manual algorithm
illustration](https://ableton-production.imgix.net/live-manual/12/OperatorAlgorithmsL12.png),
retrieved 17 October 2026, together with the user-provided global-panel screenshot.
The reference explicitly shows left-to-right colored node arrangements and the
selected ninth topology's branching edges. Unselected connectors are not drawn;
the other edges above are inferred from the hierarchical node arrangements and
must still receive native comparison/listening acceptance. The implementation
uses Hex's original `0.25`-turn edge depth and unit carrier weights; numerical,
default, sonic and preset compatibility with Live is not established.

Focused automated checks cover catalog validity, exact roundtrip/legacy
preservation, level-driven serial FM, diagram pointer/keyboard selection,
pending-choice refusal recovery and one-step canonical Undo/Redo. Native visual
and listening acceptance remains separate.

## historical compact routing matrix readouts

The previous matrix editor displayed turns and signed carrier gain in fixed-width
numeric cells. It was removed from Vector's global panel by Pulse2653. Stable
routing IDs, units, bounds, signed authored values and ID61 presets remain in the
metadata/state contract; they are not exposed as an XY editor. The obsolete matrix-specific display codecs were removed with that editor;
canonical routing domains and DSP/state tests remain. Native visual and listening
acceptance of the new algorithm/global panel remains pending.
