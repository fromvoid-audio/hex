# native device asset references

W10 / Pulse2681 / PR297 first establishes control-side reference persistence.
It does not register sampler, load sample resources into instruments, or make
asset-bearing devices playable. Sampler2679 remains the first new-device priority.

`NativeDeviceState` carries up to128 `DeviceAssetReference` values separately
from its unchanged4KiB DSP payload. Each binds a nonzero provider-owned slot to
a nonzero project-scoped asset key. Slots are unique and canonically sorted;
several slots may share an asset. This is an initial metadata-list bound, not a
sample-size, voice, zone or admitted PCM budget. No path, pointer, PCM or file
operation belongs in the neutral contract. Empty lists are represented by `None`,
with no added reference-list allocation for ordinary devices.

Project history validates references against its existing asset catalog before
preparation/commit. Snapshot restoration checks Track effects, instruments and
Return effects. Referenced assets cannot be removed. Generic state replacement
and its existing expected-state/identity-reservation command provide atomic
binding edits; a zero-ID reservation needs no new edit model. Failed or stale
edits preserve the previous project/history. Asset IDs alone do not fingerprint
file contents: changed-file verification and PCM admission remain later work.

Provider codecs frequently reconstruct their opaque state. Both registries
restore host gains and asset references before current-state validation and
after scalar/compound edits. The default metadata batch restores them between
private patches; default availability queries retain them through migration.
Providers cannot replace host bindings by returning a different metadata list.
Explicit host edits own those changes. `processing_eq` includes references while
excluding host gains; `without_host_gains` retains bindings.

## persistence and refusal

Asset-free projects retain schema29. Asset-bearing projects use schema30: the
complete strict schema29 body plus a required manifest entry for every device,
including empty lists. Missing/duplicate/unknown devices, duplicate slots, invalid
IDs, unresolved catalog assets and oversized lists refuse. Original JSON keys
are checked before adapting the old body, retaining duplicate-key rejection.
Schema20/22 writers explicitly refuse references; they cannot silently omit them.
Presets similarly refuse until a portable asset manifest/remapping exists.
Paths remain in the existing project asset catalog with original case.

This reference list has no runtime storage-ownership proof. Supported processing
therefore rejects nonempty references before codec migration, including bypassed
or unused devices. Gates cover registry effect/instrument preparation, public
prepared chains, the direct Utility constructor and its closed engine custody
path. Payload storage observations remain explicitly payload-only. Arbitrary
provider overrides called directly retain their own contract obligations;
registry admission is not a universal wrapper around third-party trait calls.
Session operations requiring audio preparation still refuse these configurations
and must preserve the prior canonical/applied state. Control-side project
save/open/Undo is not successful live sample assignment.

## verification and next slices

Focused fixtures cover canonical bounds/identity, metadata-stripping codecs,
default and custom compound edits, zero preparation calls on refusal, manual
prepared chains, concrete memoryless Utility, project history/failed preparation,
and strict schema round trips/malformed manifests. Recorded passes belong to
exact candidates; final full local CI remains required. The isolated default
lattice test command also exposed an inherited example feature declaration gap,
tracked separately as2747/PR357; focused conformance commands enable that feature
explicitly. No native interaction, listening or callback-resource proof is claimed.

Next: original-rate resident PCM preparation and bounded host leases, with unique
allocation accounting across pending/current/candidate/history/outgoing voices;
then actual resource-aware instrument borrowing and off-callback retirement.
Trigger note lifetime must distinguish onset-relative natural duration from the
existing gate-end-plus-release law across Arrangement, Session, monitoring and
export. Asset import plus assignment must become one prepared undoable edit.
Only after those paths are proved can sampler become selectable. Streaming,
portable asset presets, relinking/content integrity, IRs/tables and complete
W10/W15/W20/native/reference acceptance remain open.
