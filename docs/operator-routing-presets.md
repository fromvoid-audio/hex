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
The current default topology is distinct from all three named layouts.

The selector is derived from exact numeric equality of all twenty routing
values; it stores no extra marker and leaves the state version and payload size
unchanged. Selecting `custom` is a no-op on routing, not a persistent mode. A
manual topology edit reads back as `custom` unless the resulting matrix and
carrier weights exactly equal one of the named layouts. Editing oscillator,
envelope or waveform values does not affect the topology readback. A later
named selection replaces the current custom topology.

Tests cover exact layouts, distinct defaults, descriptor and facade readback,
atomic field preservation, idempotence, invalid choices/types, one-bit-near
classification, matrix and carrier edits, custom no-op behavior and transitions
from manual routing to another named layout. A canonical Session integration test verifies one preset edit restores the exact
original state with one Undo and the selected state with one Redo; no preset-specific storage or DSP path is
introduced.

## compact routing matrix readouts

The existing 46-pixel matrix cells show modulation in turns with a compact `t`
suffix and signed linear carrier amplitude with an `x` suffix. Numeric precision
remains two decimal places with trailing zeros trimmed. Full descriptor labels
and units (`turns`, `linear`) remain discoverable in cell tooltips; typed input
accepts the full unit, compact suffix or a bare number. Other units and values
outside the existing domains are refused. No conversion to decibels or changes
to routing, parameter IDs, domains, DSP or history are introduced.

Cells retain fixed widths during focus, pointer presses and typed edits. Long
typed drafts scroll within the cell's clipping boundary rather than painting
over neighboring columns. Rendered GPUI regressions cover both editor widths,
signed endpoints, interior negative values, zero, typing, real canonical commits
and active/focused geometry. The test platform's font system does not establish
native glyph rendering; bundled-font advance measurements and actual native
visual review are separate evidence. The private native reference capture is
unavailable in the cloud checkout and native visual acceptance remains pending.
