# Shared Lattice knob artwork — 11 October 2026

Pulse #2345 / [PR #157](https://github.com/PORTALSURFER/hex/pull/157).
Status: observed; implementation and native acceptance in progress.

## finite acceptance

- All shared knob appearances use short neutral gray corner markers while
  focused or actively edited; no continuous cyan selected-control outline.
- Knob rails fill progressively from the normalized minimum through the actual
  position. Minimum has a black rail and no cyan fill; maximum fills the entire
  usable arc. Logarithmic and bipolar laws stay owned by NumericSpec.
- Use explicit semantic ReadoutGap, BottomGap and Pan orientations, independent
  of density/scale. Match the observed lower-right class, keeping label/readout/hit geometry
  legible. Review the wider Coarse tile without blindly growing every dial.
- Preserve keyboard focus, caret, numeric drafts, drag/precision/reset/cancel,
  canonical binding and Undo. Cursor PR155 and navigation PR156 remain separate.
- Rendered focus/geometry regressions, representative multiple-device classes,
  narrow/wide layouts, same-sized before/after native captures and full local CI
  on the final commit. Required missing evidence prevents merge.

## reference provenance and observations

Installed Live rechecked 11 October: 12.4.6 (2026-09-10_0de5c8fa9a),
arm64; executable SHA256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
Existing stopped Live window was inspected read-only; one stationary click
selected Osc A Coarse Frequency, preserving its value1. No reference project,
transport, audio setup or parameter value was edited or saved.

Ignored private captures under `research/private/knob-focus-20261011/`:

- `live-tone-focused.png`: SHA256
  `0fa007cdfaf99498f30bab0a578761ecdf0a309f92ee3d2989430aa3ab5d6674`.
- `live-coarse-focused.png`: SHA256
  `42d3067cf5e7da6ffed9cf3a3493943add9be9e22ad93030f3d3e4947f550574`.

Both show four small gray L-shaped corners, including the label and readout.
They do not show a continuous or chamfered rectangle. At2x capture scale each
arm is approximately6physical pixels (3logical), with about2physical pixels
(1logical) thickness. Coarse spans roughly116x80physical pixels (58x40logical),
while its dial diameter is about36physical pixels (18logical). These are
manual raster estimates, not authoritative accessibility hitbox measurements.

Tone100% has cyan filling the full rail. Its arc begins at six o'clock and
ends at three o'clock, leaving the lower-right quadrant for the readout.
Time0% is a bipolar center value: its needle points upper-left; numeric zero
must not be treated as normalized minimum. Coarse1 is near its lower endpoint,
not proof of the exact minimum. The proposed58px MicroDial tile preserves its
18px dial diameter and40px height. Mini now shares this label-above tile;
Compact/Standard retain their footprints. The user reported a clipped Mini
"amount" label and upward mouse-down wobble in the original review fixture.
A rendered regression reproduced Standard dial height34x32 becoming34x31 and
readout y48 becoming47 when a conditional capture element inserted a flex gap.
Absolute capture positioning removes that gap; all four knob densities keep
exact tile/dial/readout bounds through down/drag/up and long typed drafts.

The user separately observed Live bottom-gap mixer/Return knobs and pan with a
center-up needle and a small arrow. Explicit semantic variants encode that
distinction; bipolar Time0 retains its observed upper-left ReadoutGap needle.
The public [Live12 Mixing manual](https://www.ableton.com/en/live-manual/12/mixing/)
supports pan triangle reset to default. Installed pan/Return screenshots and
exact endpoints/marker measurements remain pending; generic triangle artwork
does not establish reference parity.

## limits and remaining evidence

Live's actual minimum/middle/maximum sequence has not been exercised because
its current window is a user project. No generic pixel parity, Live transfer-law
or hitbox equivalence claim follows from these captures. Native Hex states,
representative multi-device layouts and exact-source CI remain required.
Reference images are private evidence and are not shipped or committed.
The original native review window remains available to the user; it predates the
Mini/wobble/semantic repairs. Updated native acceptance must use the later fixture.
The fixture columns start at normalized minimum/middle/maximum and are editable.
