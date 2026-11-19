# user-requested handoff, 19 november 2026

The user requested that all work be tracked in **hex 0.2**, pushed, and stopped
in this chat so other agents can continue without another merge participant.
Pulse project 2 / version 18 is authoritative. No further implementation,
qualification, merging or app installation is authorized for this stopped chat.

This is a recovery package, **not a merge candidate**. Pulse owns scope/status;
source doc comments own technical contracts. Do not apply all patches together.
The manifest records hashes and exact files. No user project, commercial media,
plug-in state, screenshot, binary, cache or credential is included.

## current source and verification

- Draft [PR #4055](https://github.com/PORTALSURFER/hex/pull/4055), issue
  [#8623](https://pulse.iocore.dev/#/issues/8623), head
  `2791e93abc3d1e67b0eb904cefd49a15f1ebe82e`, contains the inert fade-clock marker.
  It is already pushed. Seven quick guards passed. Cargo admission was delayed;
  the first compilation started just before the user stop and was terminated.
  **No tests ran; qualification is interrupted/incomplete. Do not merge on the
  existing evidence.** Cancellation and partial compile logs are in `8623/`.
- Draft [PR #4064](https://github.com/PORTALSURFER/hex/pull/4064), issue
  [#8635](https://pulse.iocore.dev/#/issues/8635), is the canonical intake for the
  root-normalization work. Its implementation is only in this package's draft
  patch/source, not yet applied to that PR branch.
- Both unfinished implementation issues were returned to `todo` for pickup.
  Existing completed states, dependencies and user checklist states were retained.
- The queued/running qualifier and its verified descendants belonging to this
  chat were stopped. Other executors and shared lock files were not touched.
- This chat did not install a new app or perform native acceptance.

## usable draft stages, all uncompiled and unrun

1. `outer-fade-marker/`: source patch corresponding to #8623 / PR4055.
2. `8628/`: [outer destination query](https://pulse.iocore.dev/#/issues/8628),
   based on the marker patch. Latest patch hash begins `db10f2cd`; ten drafted
   tests. Canonical and preview queries borrow existing maps. Musical author
   origin is immutable; exact signed displacement is the normalized consumed /
   prefix tick pair. Frame offset and prefix must not both be nonzero. Hidden
   knots are filtered before absolute-frame overflow. `hidden-knot-before.rs`
   preserves the pre-fix implementation for a focused before/after witness.
3. `8635/`: [private root normalization](https://pulse.iocore.dev/#/issues/8635),
   based on 2791. Latest patch hash begins `c0399011`; four drafted tests.
   Only the top Outer envelope becomes a private neutral Legacy processing
   carrier. Child/content data remains unchanged. Scope is processing Root only;
   View/Group destination finalization and native handoff are not implemented.
   Child proof is structural, stream proof is eligibility metadata, not playback.
4. `8639/`: [crop-context prerequisite](https://pulse.iocore.dev/#/issues/8639),
   based on the marker plus latest 8628. Hash begins `131141f2`; five new tests.
   Adds a private signed placement inverse, checked context helper and one real
   `trim_view` caller; existing source-only recursion avoids applying it twice.
   Nonneutral silent tails use checked span/out-end padding; neutral remains
   unity. Other split/prefix routes remain
   [#8636](https://pulse.iocore.dev/#/issues/8636). No admitted command/Undo/native
   behavior has been verified. Source copies accompany patches for reconciliation.

All stages keep Outer canonical admission/capture defaults disabled. Reconcile
against current main, run the required owner/consumer checks, and continue through
the canonical issue PRs. The handoff PR is storage, not a replacement canonical PR.

## remaining audio outcome

[Outer clip fades #8614](https://pulse.iocore.dev/#/issues/8614) remains incomplete.
The user requires **one clip fade-in at the outer start and one fade-out at the
outer end**; source repeat seams are not clip starts. Event fades still repeat.

Remaining work includes complete crop/split/prefix wiring; frame-to-musical,
freeze/unanchor, stretch, reset and alias context preservation; exact Frame
destination finalization/native handoff; bounded resident non-Frame finalization;
strict versioned saved codec; and coherent UI activation. These are requirements
of #8614/#8636, not work completed by the drafts. Do not enable Outer while known
consumers still use root fade clocks or omit the outer gain from prepared peaks.

Keep legacy writer/DTO refusal after activation: do not globally flip the marker's
`require_supported` helper to accept Outer. New codec selection must include
neutral/nested Outer domains, retain complete state and shared alias identities,
and use the existing Save As / explicit-overwrite compatibility flow. No schema
number has been reserved. Historical ambiguous trim intent remains Legacy; the
known saved test project can be copied and explicitly reauthored with unchanged
`SetClipFadeEnvelope` payload. Do not alter the user's original or unsaved app.

Stream scope was inspected: nonneutral Legacy musical/frozen Re-Pitch and granular
stream roots were already unsupported. Preserve existing resident fallback/refusal;
neutral Outer must retain supported stream shapes. Do not introduce a callback
TempoMap evaluator or silently downgrade Outer to Legacy. Plain Frame pitch is
an existing faded-stream exception; its sealed native slot needs separate review.

Final regression must exercise real Editor commands and compiled PCM for four
19174-frame cycles / 76696-frame outer duration, with event fades repeating and
clip fades once. A command-based pre-activation failure and post-activation pass
is stronger than only query assertions or manually attaching an engine fade.

## UI and piano roll

- [#8582](https://pulse.iocore.dev/#/issues/8582),
  [#8590](https://pulse.iocore.dev/#/issues/8590),
  [#8594](https://pulse.iocore.dev/#/issues/8594): existing persistent shade work
  followed the old repeated-root law. The new definition supersedes that law.
  Playback, idle/held/pending shade, geometry keys and delayed ACK must agree.
- [#8584](https://pulse.iocore.dev/#/issues/8584),
  [#8588](https://pulse.iocore.dev/#/issues/8588),
  [#8611](https://pulse.iocore.dev/#/issues/8611): live waveform work remains.
  `8588/` contains old raw/event/Arrangement binding prototypes. **They use the
  superseded root fade law and must not be applied unchanged.** Root/event view
  has no unique varying Outer envelope. Exact timeline reads retain destination
  age; coarse range unions must use original destination-column context. Preserve
  source/key identity across ACK and do not add duplicate resident PCM roots or
  decorative overrange markers. `8594/` is also superseded research code.
- [Piano parent #8601](https://pulse.iocore.dev/#/issues/8601):
  [#8602](https://pulse.iocore.dev/#/issues/8602) Z selected/all-notes fit and
  [#8606](https://pulse.iocore.dev/#/issues/8606) fractional SceneKey are merged.
  [#8604](https://pulse.iocore.dev/#/issues/8604) still needs full Editor/capture/
  paint state migration; [#8605](https://pulse.iocore.dev/#/issues/8605) needs
  smooth left/right piano-key zoom, closer defaults and screen-based limits.
  `8604/partial-prototype.patch` is incomplete and predates final SceneKey fixes.
  Do not restore its per-note f64 geometry (measured regression) or its unclamped
  inverse. Preserve current signed-Q/f32 forward geometry and bounded inverse.
  Z must still fit all notes even beyond normal manual readability limits.

## already merged and pushed

- Diagnostic #8581 / PR4021; persistent shade #8582 / PR4023.
- Filled scope bars #8583 / PR4026.
- EQ Arrangement-wrap continuity #8586 / PR4028 (original click parent #8581).
- Old root-phase frame shade #8590 / PR4034; its law now needs correction.
- Z selected/all notes #8602 / PR4037.
- Raw source query #8584 / PR4038; its old top fade law needs correction.
- Fractional SceneKey #8606 / PR4042.
- Runtime fade-slot/sparse prerequisite #8616 / PR4048, merge
  `970d8ecbf9866f091ad8a1984c6e8445ea2a893b`. Evidence in `8616/` distinguishes
  fresh checks, carried unchanged-source checks and timing. This alone did not
  activate the outer clip fade law.

All 21 issue IDs are in `manifest.json` and moved to hex 0.2 at handoff. Other
threads should pick up the existing issues/PRs. Do not create duplicates or treat
an archived local prototype, a merge or a disk build as native acceptance.
