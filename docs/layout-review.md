# Native layout review catalog

Pulse #2355 / PR #163 owns the disposable Rust/GPUI `layout_review` example.
It opens a separate window, loads/verifies embedded Ioskeley Mono through the
production theme loader and renders app text through `theme::text`. It creates
no audio device, project, transport, worker or preferences file. Existing Hex,
Live and knob review windows must remain untouched.

Run from this checkout after the coordinated Mac build slot is granted:

```sh
cargo run --locked -p hex-ui --example layout_review
cargo test --locked -p hex-ui --example layout_review --features ui-test-support
```

The example reuses production `device_frame`, `compact_device_frame`,
`panel_tab` and device corner/header geometry. Rows/display compositions and the
header-only hit overlay are explicitly fixture-owned candidates. No production
layout or keyboard contract is changed. PR156 owns Operator layout/navigation,
PR157 owns knobs/shared control styling, and PR158 owns harmonic editing.

## Finite inventory and confidence

| Family | Evidence | Meaningful states and fixture interaction |
| --- | --- | --- |
| Standard device frame/header | User whole-device crop; production shared frame | Idle/focused/selected/disabled. Only the header selects the device; cyan selected header is independent of section selection. |
| Compact device frame/header | Existing production compact frame | Same fixture interaction; compact header density is a Hex variant, not independently verified Live geometry. |
| Rounded dark display | User dark-gray center crop | Neutral `0x191919` center; rounded outline; static guide only. Focus/selection border are fixture inspection aids, not claimed Live behavior. |
| Section rail | User left/right crops; PR156 source tokens | Rectangular `0x363636` rows, `0x474747` gutters; Focused and selected rows merge into `0x474747` outer background; focus retains a border marker. Two-logical-pixel gutters (four physical pixels at scale 2). Single selected section. |
| Grouped rows | User oscillator rows | Fixed labels/readout footprint; focus and selection merge the first row into the outer background in previews. No numeric control or oscillator semantics. |
| Stable tabs | User center crop; shared `panel_tab` | Fixed three-tab footprint, independent focus versus active page; changing page does not change panel size. Exact Live tab marker/underline geometry remains unverified. |
| Compact grouped sections | User modulation/filter/pitch/global section crop | Dense separated sections, shared row state policy. Text-only layout placeholders have no audio effect. |

User-supplied crops were preserved by the navigation coordinator at
`research/private/operator-navigation-20261011/` in its `bd8f` checkout:
`user-left-rail.png`, `user-right-rail.png`, `user-dark-gray-center.png`,
`user-selected-device.png`. These private reference assets are not included in
Git or the example. They show appearance, not a recorded keyboard experiment.
The installed reference was rechecked on 11 October 2026 through its Info.plist:
Live 12.4.6 `(2026-09-10_0de5c8fa9a)`. No Live input or new binary analysis was
performed. A later direct user clarification and whole-device crop
`user-header-row-clarification.png` (retained in this checkout
`research/private/layout-review-20261011/`) require the idle device header to
merge into the gray surround without a dark separating border, wider row gutters,
and both focused and selected rows to use exactly the outer background instead of a remaining
dark rectangle. The gallery applies this contract; cyan device selection remains
independent. Pixel colors/geometry are provisional tokens, not calibrated reference
measurements; PR156's current center/row/gutter values inform the candidates.

Keyboard choices (Tab/Shift-Tab cyclic target focus, arrows, Enter/Space selection,
Escape clear) are original fixture contracts. Click selection has one owner per
sample, separate from focus. Disabled samples retain selection but refuse pointer
and Enter/Space changes; focus may remain for inspection. State previews are
noninteractive and labelled, so their focus marker is illustrative. The selected
preview changes target zero; disabled previews affect all targets.

## Acceptance checklist and evidence

- [ ] Targeted rendered GPUI tests pass: header-only selection, pointer/keyboard
  transitions, disabled refusal, state geometry, preview containment/non-overlap
  at 760×660 and 1320×660 logical pixels.
- [ ] Full local CI passes on the final exact commit/unchanged source digest.
- [ ] Separately named native bundle with independent identity is runnable.
- [ ] Retain native screenshots beside private reference crops and record scale,
  window bounds, source/binary hashes and user comparison findings.
- [ ] User checks all seven families, click/Tab/Shift-Tab/arrows/Enter/Space/Escape,
  header versus body clicks, disable/enable, narrow/wide resize and available
  display scales. No automated desktop input runs alongside the user's testing.

GPUI's test platform uses scale 2; source/rendered tests do not prove native
raster appearance, physical input or other display scales. Screenshot/native
acceptance and all automated checks are pending at initial source creation.
Acceptance required by ticket2355 blocks gallery completion/merge.

Remaining reference families are explicit gaps: racks/chains and nested headers,
folded devices, plugin windows, browser/mixer/Arrangement/Session/clip-editor
surfaces, popups and parameter controls, disabled/bypassed reference transitions,
accessibility and exact per-device display geometry. This extensible catalog
covers the currently requested device-layout families; it does not claim every
Live screen or musical behavior. Add a family only with bounded evidence and
acceptance, rather than silently expanding this PR.
