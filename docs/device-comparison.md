# shared device a/b comparison

Implementation contract under review for Pulse2845 / PR447. This document is a
source-backed proposal, not a statement that the feature is implemented or
qualified. Audited source: main `bc06d7573ffa7f4d989bdff9553ed3108f4f0818`,
canonical intake retained in source checkpoint `38b008c859f3662787f3757a865071eebeec72be`.

## current boundaries

- `hex-project/src/device.rs::Device` owns a stable ID, one authored
  `NativeDeviceState` and a separate bypass flag. Its `state()` is consumed by
  every existing registered instrument/effect, including Return effects.
- `hex-lattice-contract::NativeDeviceState` owns immutable opaque payload and
  asset-reference Arcs plus host input/output trims. Its 4096-byte payload limit
  does not represent PCM, voices, transport, routes or global tuning.
- `hex-session::Session::{preview,commit}_device_parameter` patches only this
  authored state through the class-specific registered definition and existing
  prepared transaction. Failure must precede canonical history/publication.
- `DeviceSnapshot` and the shared project storage Device DTO carry one state.
  Generic preset schema1 stores class/type/version/payload, deliberately preserves
  the target device's host trims, and refuses asset-bound presets without a
  portable asset manifest. It does not currently store an A/B pair.
- Project automation currently owns track gain envelopes/lanes. This feature
  must not invent a device-parameter automation family or claim one exists.
- `computer_keyboard.rs::NOTE_KEYS` includes plain `p`; device comparison must
  consume it only for the exact device-card focus owner, leaving musical input
  elsewhere. Text, IME, numeric drafts, graph controls and dialogs take priority.

## proposed canonical representation

Keep `Device::state()` as the active authored state. Add one optional inline
comparison record containing the active slot and the inactive authored state.
Absent comparison means A is active and B starts identical. First switch captures
A by immutable state clone; it does not copy PCM or construct another DSP owner.

Parameter edits continue to update the active state through existing commands.
Switch swaps settings, retaining both variants. Copy updates the inactive variant
without switching or changing the audible settings. Both are typed canonical
commands with one Undo entry, admitted through the existing prepared path.
Different-provider replacement resets comparison; moving preserves the pair;
duplicating gets a fresh DeviceId and independent authored edits.

Expose a borrowed control-side iterator over the actual active and inactive
NativeDeviceState fields. Project media-reference validation and media deletion
checks must include both. Inactive references retain catalog identities; they do
not themselves authorize native resource admission or create media copies.

Current/Undo/Redo, project snapshots and the current-format persistence contract
must retain both variants and the active slot. No historical-alpha reader or
migration is required. No existing state-size or resource cap is raised.

## decisions and writer coordination still required

1. **Host settings:** the recommended contract compares provider payload and asset
   bindings while bypass and host input/output trims remain device-wide, matching
   existing preset semantics. All stored variants must reflect current global
   trims so switching cannot restore stale values. Slot-local host trims would
   change that preset policy and require an explicit alternative contract.
2. **Actual custody:** Source4's published Sampler checkpoint `4372a09d` has
   `audio_storage/canonical_state.rs::project_native_states` visiting only
   `device.state()`, including current/Undo/Redo through the retained-project
   iterator. Source4 must include both actual fields in its existing ledger and
   aggregate before resource-backed A/B can be called integrated. This is not a
   new constructor, parallel ledger, ownership family or admission shortcut.
3. **Asset persistence:** the audited shared project Device DTO has no asset
   reference fields, while presets explicitly refuse asset references. A/B must
   never silently omit inactive references. The current-format asset persistence
   owner/contract must be coordinated with Source4 rather than being silently
   broadened into a portable-media implementation here. Existing portable-preset
   refusal remains visible for either slot; unmerged Sampler readiness is separate.

Source1 proposes Project's comparison/model/snapshot/reference hunks, shared
Storage Device/preset codec hunks, bounded Session comparison/preset request
hunks, and a private shared UI module. Source4 retains all Engine/resource/
Sampler/compiler/custody writes. Source5 retains EQ body/filter/response behavior.
Shared UI writes are limited to instrument/effect header actions, existing
ChainWindow focus lookup, module/binding/action wiring; preserve the separate
PR402 neutral-focus and held-delete changes. No second selection owner.

## transaction and focus requirements

A request captures device ID, document generation, revision, provider class/type
and expected active slot. Recheck freshness at the canonical boundary. Existing
numeric/device gestures must commit or visibly refuse before comparison. Do not
silently retry against another device or slot, skip failed gesture admission, or
change both history and a different project after a deferred terminal event.

The active indicator and context menu share one renderer. A shows only
`switch to b` / `copy a to b`; B shows only `switch to a` / `copy b to a`.
Plain P follows the same canonical operation for the exact focused card. No
repeated-key retargeting, text/IME shortcut leak, forced focus on render or loss
of the current selection/context when the menu closes.

Active playback replacement uses existing runtime-inheritance/publication policy.
It must preserve the transport, routing and global tuning. Provider-specific
runtime continuity is tested through the existing handoff, not by copying voices
or creating a comparison-specific callback path. Refusal retains the last valid
sound and both authored states. Copying inactive metadata must not manufacture a
new audible state or native resource owner.

## finite verification plan

- Model: initial identical variants; A-edit/B-edit/repeated switch; direction-
  correct copy without switch; one Undo/Redo per command; same-type edits versus
  different-provider replacement; duplicate/move/remove/stale identity; both-slot
  asset-reference validity and media-deletion protection.
- Storage/presets: both variants and slot survive current-format round trips;
  malformed mismatched-provider state refuses atomically; both-slot asset preset
  refusal preserves existing files; host-trim behavior follows the chosen policy.
- Session: representative registered instrument, track effect and Return effect;
  registered validation and preparation refusal preserve Project/history/audio;
  gesture finalization, stale generation/revision/slot and admission backpressure.
- UI: actual rendered menu/P actions and indicator, text/numeric/IME/dialog/
  musical-keyboard isolation, focus/menu lifecycle and deleted targets, using
  bounded disposable editor fixtures before worker startup.
- Audio: real prepared render and existing inheritance preserve transport and
  held-note/tail behavior where supported, with explicit limits; no callback
  allocation/I/O/lock/owner-drop claims without the corresponding real test.

Sole merger owns prospective-candidate full local CI/review/integration/merge.
Coordinator owns finite native/optimized checks and all user-app delivery. A source
handoff or passing fixture is not installed, visually accepted or user-approved.
