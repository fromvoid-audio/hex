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

## integrated source evidence and native checklist

44 widget tests pass, including corner bounds/hit regions, normalized/log/bipolar
rails, pan marker default reset/disabled behavior and fixed press/drag/type bounds.
399 UI-feature tests pass (2 existing ignored); strict widget/UI-feature Clippy and
architecture, UI style and GPUI patch guards pass. Operator consumes exported
58px tile metrics: shell246, preview264, frame832 logical pixels. Its fixed rows
are checked at880 and1280px windows, without shrinking controls. Inline routing
fields remain46px. Utility Balance explicitly selects Pan through its display
codec; MidSide stays ReadoutGap, despite both having bipolar domains. EQ8 Mini
and existing Compact effect panels pass their integrated source tests.

The separately named `artifacts/hex-knob-review-v2.app` contains generic
five-row density/semantic examples and does not replace the user's original
open review app. Native review must establish:

1. Repeated down/hold/drag/release on each row: no dial/readout movement; Shift,
   release outside, typing a long draft, Enter and Escape still work.
2. Complete Mini "amount" label, legible corners and values at narrow/wide sizes.
   Columns start at normalized min/mid/max and remain independently editable.
3. Pan center points up; left/right fill leaves center neutral; triangle resets
   to neutral. BottomGap and ReadoutGap openings remain visibly distinct.
4. Actual Operator Coarse/Fine/Level, EQ8 Mini and Utility Balance plus another
   Compact effect at narrow/wide sizes: labels/hits/focus/typing/reset and Undo.
5. Installed corresponding Live knobs at min/middle/max, including pan triangle,
   bottom-gap Return/send class and Mini label convention. Compare same scale.

Full final-commit local CI and dependency PR155/156 acceptance are required
separately. This source evidence does not establish native event delivery, exact
Live geometry, reference transfer laws, listening or physical audio deadlines.

## later integrated native checkpoint

The published navigation dependency's gray outer rails and explicit black center
are integrated from5cc328f. Its coordinating chat retains separate acceptance
ownership. Shared knob metrics/semantics remain the only PR157 runtime additions.

Updated v2 idle capture SHA256
`0f775c90fa4dc7b2ccc3ceb25fdfc290a36eaffb5d54a63e67a9fb1642faf7fb`
shows the complete Mini amount label, generic headings and distinct semantic
rails. After explicitly approved isolated input, native typed12345.67 commits
with Enter; caret and four corner marks remain visible. The pan marker resets
76.08 to0 with a vertical upward needle and no center fill. Captures are in
`research/private/knob-focus-20261011/hex-v2-typed.png` and
`hex-v2-pan-reset.png`. CUA knob/readout drag delivery did not reliably change
values, so native held/drag acceptance is inconclusive; manual repetition is
requested. These states do not prove every gesture or narrow/wide acceptance.

The current Live binding exposes the user's E05 set, not the disposable fixture.
Only read-only AX/screenshot observation was performed. No open/switch/save,
parameter, transport or audio changes were made. The visible Arrangement numeric
mixer is insufficient for the required knob endpoint comparison. The copied
fixture remains private, but its safe active window must be established by the
human before Live endpoint/Return/pan/Mini interaction research can continue.

A separate actual-device no-audio review wrapper uses the existing public
hex_ui::open_editor entry point with unavailable audio, and does not change
production sources. Its ignored package shares every dependency version/checksum
with workspace Cargo.lock; only the fixture package is added. The unlaunched
4f0c4c4 actual-device bundle must be refreshed to the final integrated source
before actual-device screenshots are accepted.

All16 localCI steps passed on prior4f0c4c4 and unchanged source digest. The
later gray-rail integration run20261011T091500Z-3c0104ce stopped after workspace
tests at the2GiB disk floor; it is incomplete evidence. Only this chat's inactive
incremental cache was removed after verifying sole target ownership and no
active build. Final exact-source CI remains required after black-center
integration. PR157 stays draft until required native/reference and dependency
acceptance gates are met.


## human acceptance and dependency repair checkpoint

The human confirms stable geometry and complete labels in the v2 fixture for
repeated mouse-down/hold/drag/release and long typed edits across every row.
This accepts the bounded wobble and Mini label repairs; actual-device narrow/wide
review and the corresponding Live endpoint comparison remain open.

All 16 full local CI steps passed on clean commit
`6fceb49c552a3f0e0c6e340da47b93def32b166c`, with unchanged source digest
`5ef7d74e0e303df0e44a52246462cb61832e151b61efc1f10b39137bbcaf934b`.
Evidence: `artifacts/local-ci/20261011T092535Z-ae9de1e8/summary.json`.
The actual-device no-audio wrapper was refreshed from that source, remains
unlaunched, and has executable SHA256
`5c0d2188b51da6a5b66520629b609106ba7241579db5d441873c5df13f9258d3`.

The human established a disposable Live set. Fresh AX inspection shows Untitled,
an empty four-track Session with Return A/B, Play0 and audio off. A neutral
mixer capture is retained privately. AX pan setValue is unsupported; click/Home
and typed endpoint attempts left pan at0. A subsequent Raise/drag attempt was
blocked before delivery by a fresh user-input guard. Native automation paused;
no endpoint sequence or reference parity is claimed.

The later human keyboard check found Tab lost knob focus and Enter did nothing.
PR156 owns the focused keyboard repair, now integrated from
`68e25a18a752f5e38bcb854dfb4e0d82da390f75`: dedicated Routing Enter/Space
bindings and a regression using production editor bindings. New user reference
crops supersede the black-center choice with rounded neutral-dark-gray center
and dark rail rows. Shared knob metrics and Balance Pan selection are preserved.
The combined tree requires fresh full CI and native acceptance; the prior green
run is checkpoint evidence only. Separate layout-family fixture work is Pulse2355.
