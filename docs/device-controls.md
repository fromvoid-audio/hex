# Native lattice device controls

Pulse ticket 44 owns the reusable GPUI toolkit, ticket 65 owns bounded live
parameter gestures and grouped undo, and ticket 17 owns the real device panel.
The first toolkit implementation is source-complete: 14 targeted widget tests,
strict crate Clippy, architecture/style guards, formatting and desktop build
pass. All 14 full local CI steps passed on clean `7f0bdea` in 163.297 s
with unchanged source digest and `stale=false`
([PR #24](https://github.com/PORTALSURFER/hex/pull/24)). Native acceptance
remains open.

## Boundary

`hex-widgets` owns knobs, horizontal/vertical sliders, editable numeric values,
toggles, selectors and compact device frames. It depends on pinned GPUI and std,
with no canonical project, DSP, storage or device-I/O dependency. The caller
supplies validated display specifications, formatting/parsing, lowercase text,
style and event bindings. App composition inherits the verified Ioskeley Mono
font and routes text through `hex-ui::theme::text` before passing it to widgets.
Inject colors so the planned four-color theme does not require a second palette.

Control entities retain local draft values and focus across renders. They emit
begin/change/commit/cancel events; they neither write files nor create history.
Device descriptors drive general controls; adding a device does not add storage
branches or require a competing UI editing model.

The device panel resolves a bounded registered-type catalog for its add menu
and a complete descriptor/value snapshot per changed device state. These
control-side engine APIs resolve and migrate once per snapshot, rather than
looking up every knob separately. Optional device-owned display names are
presentation metadata; stable type IDs and opaque persisted state are unchanged.
Unknown or unsupported states report errors instead of displaying defaults.

## Interaction contract

- Validate finite ranges/defaults and typed input. Linear, positive logarithmic
  and zero-centered bipolar mappings have explicit domains and inverse mappings.
  Clamp pointer adjustments; refuse invalid typed values without changing state.
  Bound numeric text and IME replacements before shaping. Modifier changes
  during a drag must preserve the value at the transition.
- A stationary click on a value enters text editing; dragging adjusts it after
  a movement threshold. Knob double-click resets to the supplied default.
- Capture an active pointer gesture through release outside the control bounds.
  Escape cancels; Enter commits text; focused arrow keys adjust values. Text
  selection, clipboard and IME remain ordinary focused input behavior.
- Supply immediate local feedback. A stale external value must not overwrite an
  active draft; the controller handles canonical/audio acknowledgement identity.
- Keep compact knobs, thin slider tracks with usable hit regions, small value
  rows and contiguous device frames. Preserve readable text and visible focus.

Shift fine adjustment is an initial original interaction policy. Exact Live
transfer laws, modifier behavior and numerical increments are unverified until
reference experiments establish them. The visual target is the user-supplied
Live device screenshot; rendering and assets are independent implementations.

## Controller dependency

The parameter actor admits one active gesture per project. Transient changes
prepare an overlay on the latest canonical state without mutating history.
Commit includes the final typed value and performs one prepared canonical edit;
cancel restores the latest canonical sound. Independent parameter edits are
retained. Preview source identity must distinguish project generation, gesture
token and sequence from canonical revision. Coalescing cannot discard terminal
commit/cancel or weaken bounded queue ordering. Save/export finalize before
capture; Undo finalizes and then undoes. Target deletion/project replacement
cancel the overlay. This is ticket 65's contract, not completed toolkit support.
Close and Quit must also protect a changed local draft whose canonical project
is still clean, including a retained terminal request. Existing Cancel and
Discard decisions remain authoritative.

## Acceptance

Source checks cover mappings, malformed numbers, drag cancellation/commit,
focused text input and actual rendered control events, followed by clean local
CI. Native device-panel interaction and listening belong to the user's review
build: fine adjustment, click-to-type, reset, outside-bound release, keyboard/IME,
undo, reload and compact minimum-window geometry. Automated checks do not prove
native input feel or measured one-to-one Live compatibility.

The targeted suite also covers fine-adjust modifier transitions, exact logarithmic
endpoints, reset event ordering, disabling an active drag, focused shortcut
consumption, 256-byte text/IME limits and UTF-16 marked-range conversion. These
headless rendered checks do not replace the native checklist above.
