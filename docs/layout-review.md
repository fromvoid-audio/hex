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
| Standard device frame/header | User whole-device crop; production shared frame | Idle/selected/disabled; keyboard focus uses the selected appearance. Only the header selects the device; cyan selected header is independent of section selection. |
| Compact device frame/header | Existing production compact frame | Same fixture interaction; compact header density is a Hex variant, not independently verified Live geometry. |
| Rounded dark display | User dark-gray center crop | Neutral `0x191919` center; rounded outline; static guide only. Selection uses the same appearance for pointer and keyboard focus; the cyan screen marker is a fixture inspection aid. |
| Section rail | User left/right crops; PR156 source tokens | Rounded `0x363636` sections, `0x474747` gutters; Focused and selected rows merge into `0x474747` outer background; there is no separate white focus border. Two-logical-pixel gutters (four physical pixels at scale 2). Single selected section. |
| Grouped rows | User oscillator rows | Fixed labels/readout footprint; focus and selection merge the first row into the outer background in previews. No numeric control or oscillator semantics. |
| Stable tabs | User center crop; shared `panel_tab` | Fixed three-tab footprint, independent focus versus active page; changing page does not change panel size. Exact Live tab marker/underline geometry remains unverified. |
| Compact grouped sections | User modulation/filter/pitch/global section crop | Dense separated sections, shared row state policy. Text-only layout placeholders have no audio effect. |

Four additional composition families follow the direct user explanation and
Operator/Wavetable attachments from 11 October 2026:

| Family | Composition and independent interaction |
| --- | --- |
| Inset inspector | Deep-dark screen protrudes into the header; a host-colored surround cuts into the adjacent rounded sections. Screen occludes header input. Header and section selection have separate ownership. |
| Raised screen tabs | Active rounded tab protrudes into the header alongside compact page tabs; switching pages retains screen and hitbox geometry. Header selection does not reset the active page. |
| Joined screen / section | Deep-dark upper screen and lighter-dark lower control section meet at a straight seam and share a rounded outer silhouette. Lower section focus removes only its darker fill. |
| Split tall / short sections | Independently rounded long and short sections separated by a host-colored gutter; each can merge into the host independently. |

Every device host now has a light-in-context rounded outer surface, with an idle
header of the same color. Normal section focus/selection uses exactly the host
fill, while inspector screens retain their deep-dark fill. Whole-device header
selection remains cyan, independent of section selection or active screen page.
These are fixture-owned compositions; this PR does not alter production device
layouts. The palette currently uses neutral reference-review roles; arbitrary
user theme preferences are not loaded by this disposable example.

The earlier `35c6541` report incorrectly stated that focused rows already used
the exact host fill: source inspection found the residual `0x414141` branch.
This update corrects that branch and replaces the old app/CI evidence. Geometry
and interaction tests do not prove painted colors: GPUI's test platform discards
raster output. Native reference comparison remains an explicit acceptance gate
on Pulse2355, including this focused-fill regression.

New private copies in this checkout at
`research/private/layout-review-20261011/` retain the user attachments:
`user-wavetable-composition.png`, `user-operator-inspector.png`, and
`user-operator-selected-header.png`, and `user-utility-enabled-disabled.png`. Appearance and composition are directly
observed/provided; keyboard behavior is still the fixture's original contract.

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
noninteractive and labelled; there is no separate focused preview. The selected
preview changes target zero; composite previews also select section one.
Disabled previews affect all targets.

## Acceptance checklist and evidence

- [ ] Renew targeted rendered GPUI tests after the latest user corrections: header-only selection, pointer/keyboard
  transitions, disabled refusal, state geometry, preview containment/non-overlap
  at 760×660 and 1320×660 logical pixels.
- Full local CI must pass on the final exact commit/unchanged source digest;
  its retained summary and result are recorded on PR #163 and Pulse #2355.
- Separately named native bundle with independent identity is prepared after
  the final native build; its path and binary hash are recorded on PR #163.
- [ ] Retain native screenshots beside private reference crops and record scale,
  window bounds, source/binary hashes and user comparison findings.
- [ ] User checks all eleven families, click/Tab/Shift-Tab/arrows/Enter/Space/Escape,
  header versus body clicks, disable/enable, narrow/wide resize and available
  display scales. No automated desktop input runs alongside the user's testing.

GPUI's test platform uses scale 2; source/rendered tests do not prove native
raster appearance, physical input or other display scales. Screenshot/native
acceptance remain pending. The previous four-test pass and native build apply to the superseded
`35c6541` source. The expanded source now passes six example tests (four rendered
gallery regressions plus two theme tests), strict example/tests Clippy and the
native example build. Inspector/header overlap and surround, occluded screen
input, independent header/section/page ownership, fixed page footprints and all
eleven-family pointer/keyboard/disabled/minimum-wide geometry checks are covered.
Final full CI evidence is retained with the published exact commit/digest on
PR163/Pulse2355; native visual/reference acceptance remains open.
The first compile exposed the pinned GPUI one-argument focus API; the fixture
was corrected before these passes. Final local CI results live with the retained
commit/digest evidence rather than being inferred from the targeted checks.
Acceptance required by ticket2355 blocks gallery completion/merge.

Remaining reference families are explicit gaps: racks/chains and nested headers,
folded devices, plugin windows, browser/mixer/Arrangement/Session/clip-editor
surfaces, popups and parameter controls, production disabled/bypassed transitions,
accessibility and exact per-device display geometry. This extensible catalog
covers the currently requested device-layout and composite families; it does not claim every
Live screen or musical behavior. Add a family only with bounded evidence and
acceptance, rather than silently expanding this PR.

### Latest user corrections — 11 October 2026

The user accepted the composition direction and clarified that device focus and
selection have the same visual appearance: a highlighted header, with no white
device outline. The separate focused preview is removed; idle, selected and
disabled remain. Keyboard focus is retained for navigation and paints selected
section/header appearance without changing the committed selection.

The supplied side-by-side Utility crop requires an orange enabled activator and
a dark disabled activator. Disabled labels, readouts and static screen guide
marks are muted in every family. Disabling still refuses sample input and
preserves geometry. These changes are fixture-only; they do not activate or
bypass audio devices. The existing eleven-family input regression now verifies
that disabled appearance overrides retained focus/selection, and a new rendered
regression exercises keyboard focus with no separate focused preview.

The previously validated `ad51d79` app and CI are superseded for this candidate.
Source guards pass; renewed targeted tests, native build, exact final full CI
and human visual acceptance are pending the coordinated Mac build slot.
