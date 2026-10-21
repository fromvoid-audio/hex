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
file contents. Current schema34 additionally persists optional expected original
content facts; the narrow sampler save/open path reconstructs actual native
banks and compares the existing catalog law before adoption. These facts grant
no PCM/native authority. See [sampler save/reopen](sampler.md#save-and-reopen-original-sources--pulse2906--pr499).

Provider codecs frequently reconstruct their opaque state. Both registries
restore host gains and asset references before current-state validation and
after scalar/compound edits. The default metadata batch restores them between
private patches; default availability queries retain them through migration.
Providers cannot replace host bindings by returning a different metadata list.
Explicit host edits own those changes. `processing_eq` includes references while
excluding host gains; `without_host_gains` retains bindings.

## persistence and refusal

The original combined schema31 first required tuning and device assets, over
the complete strict prior body, with a required manifest entry for every device,
including empty lists. Missing/duplicate/unknown devices, duplicate slots, invalid
IDs, unresolved catalog assets and oversized lists refuse. Original JSON keys
are checked before adapting the old body, retaining duplicate-key rejection.
Schema20/22 writers explicitly refuse references; they cannot silently omit them.
Presets similarly refuse until a portable asset manifest/remapping exists.
Paths remain in the existing project asset catalog with original case. Superseded
alpha schema30 refuses explicitly, regardless of its field combination; opening
does not convert or rewrite it. Existing readers through29 remain unchanged.

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

## user-paused cloud checkpoint

The user paused all local work on17 October2026, then authorized focused
checkpoint pushes and primary-dispatched cloud continuation. Implementation
465d95c761e3c38671792ec25e11bbd7dfbca09f is pushed on canonical PR297.
This note changes no implementation. No local compiler/test process remains
owned by this task; do not automatically resume local checks or installation.

Development checks: contract/lattice with conformance enabled and the initial
three storage/history tests pass. The latest chain/custom-override/concrete
Utility/closed Utility/Session tests and complete final candidate checks are
still unverified. Initial independent implementation review found no actionable
findings. Raw development logs remain in the Mac worktree under
artifacts/device-assets/development/ and are not automatically cloud-accessible.
No full W10 CI run or live sampler delivery has occurred.

Integration warning: parallel tuning issue2739, observed sourceac29a1ad in
worktree0bed, also introduces storage schema30 in schema/tuning.rs. W10's
asset-reference schema30 must be reconciled with that canonical work before
merge, preserving both strict migrations and all data. Do not publish two
incompatible schema30 formats or discard tuning/references. W05/W06 integration
and fresh exact-source checks also remain required. The cloud primary owns
dispatch and ordering; this Mac remains paused until explicit user resume.

## cloud integration — 17 october 2026

Integrated current W05 `0acd77e6` into this canonical branch at `f3008e71`.
Conflict resolution retains host asset metadata before semantic validation and
after every private default batch patch. Both registry suites retain asset
preservation, preparation refusal and read-only/invalid-semantic coverage.
The semantic refusal fixture also carries asset bindings and checks unchanged
authored state. Storage schemas were not changed by this integration.

At `f3008e71`, the contract, lattice, project and storage suites pass 422 tests
with no failures or ignored tests, explicitly enabling lattice's conformance
feature. The host checks then exposed a missing exhaustive engine error-map arm
for `AssetResourcesUnavailable` (E0004). Repair `83e94448` adds its explicit
lowercase refusal message. The repaired device/engine/Session asset checks pass
six cases, including direct/closed Utility and actual Session preparation with
unchanged project/history after refusal. Strict all-target Clippy passes for
the seven affected crates with the same conformance feature. Logs, including
the initial compile failure, are retained in `artifacts/device-assets/cloud4/`.
These checks do not constitute full final-source CI or callback lease proof.

Cloud4 and cloud5 agree on the integration order recorded in Pulse2739 notes:
W10 retains asset schema30, then tuning PR350 owns combined schema31, requiring
both tuning and the complete device asset manifest. Cloud5 is the single writer
for the combined storage integration. The user's subsequent active-alpha policy
supersedes the proposed historical schema30 compatibility readers: no conversion
or migration between these unpublished formats is required. Both active data
families must survive in one coherent current format, with strict validation,
save/reopen and Undo integrity and explicit unsupported-format errors. Existing
user files must not be automatically rewritten or deleted. This does not remove
existing migration code as unrelated cleanup. The separate AGENTS policy change
is Pulse2758/PR366. Exact final-source checks remain required.
Mac verification/implementation requests now go through the coordinator's
serial local queue; this cloud task does not start local work itself.

The first integrated full run at `6ed1e8fc` passed source checks but failed five
Operator storage-proof tests: the optional asset-reference slice adds16 bytes
to `NativeDeviceState`, including asset-free states. The actual shared context
charge is now3008 bytes and the capture-proof context charge2628. Under the
unchanged2MiB retained limit, complete typed recipes fit409 entries and inline
recipes412. Boundary tests derive their count from actual charges and still
require the next recipe to refuse without mutation; concrete layout assertions
retain the measured current sizes. No production budget or runtime limit is
raised. All136 Operator-state tests pass after this proof repair. The failed full
receipt remains `artifacts/local-ci/20261017T132900Z-db43c519/summary.json`;
final exact-source full validation must be rerun after the repair.

The next full run at `1414d69d` exposed the corresponding engine admission
boundary assertion:425 complete companion-context recipes fit, rather than426,
under the unchanged2MiB ceiling. Its computed next-recipe refusal remains.
The earlier device-state proof repair is retained; this engine assertion and
its owning README now report the current asset-metadata charge. The failed run
is `artifacts/local-ci/20261017T133404Z-073b6e04/summary.json`.

## original-rate PCM retention — pulse2762 / PR370

`PreparedImport` now retains the exact original-rate decoded stereo `Audio`
alongside the existing project-rate clip derivative. Single and batch imports,
Arrangement tracks/clips, Session cells and Open/recovery keep both in the
control-side Session bank. `Session::audio` continues returning the unchanged
clip derivative; `Session::source_audio` borrows the original for future sample
preparation. Existing decode expands mono to stereo explicitly; original-rate
here describes decoded PCM, not untouched encoded file bytes. Paths/source
metadata and waveform summaries keep their existing meanings.

The same caller resident-frame ceiling now charges the unique PCM buffer
capacity across retained sources/derivatives and aliases, using exact owner
identity rather than sample equality. `Audio::pcm_capacity_frames` observes the
buffer's allocated frame capacity, so excess Vec capacity cannot evade the
frame bound. Identity conversion clones the same PCM owner and incurs no second
buffer charge. Loading a known identity alias still works when remaining frame
capacity is zero. Fresh decoded files allocate distinct buffers and remain
charged individually even when their content/path matches.

Batch workers and Open/recovery decrement their remaining retained-frame bounds
for original plus converted buffers before retaining another prepared member.
Over-budget candidates refuse before project/history/applied replacement;
Undo/Redo retain the exact original source without reopening the file. The
existing ceiling is not raised. Consequently some converted imports that fit
only the derivative under the old accounting can now refuse explicitly.

This is Session-bank PCM **frame** accounting, not the complete graph/resource
allocation-byte union or handle/lease/retirement proof. It excludes owner/graph
metadata and pending work outside this bank; those remain required in the shared
resident-resource admission path. Borrowing `source_audio` grants no callback
lease and nonempty device bindings still refuse processing. No tuning/schema,
Lattice API, registration or user-file rewriting is added by this slice.

All113 Session library tests pass, including four direct original/derivative,
alias/capacity, Open/history, failed-batch and Session-cell fixtures. Earlier
mixed-rate assertions now count the retained original; an actual identity-alias
Open refusal was reproduced and repaired. Focused logs retain both failures
and passes. Final exact-source full validation remains required.

The first full run at `cefb4ff5` exposed the mixed-rate service fixture's old
441-frame derivative-only reservation. It now explicitly proves that budget
refuses without project/unsaved-state mutation, then exercises exact save/reopen
and changed-source refusal with the actual480+441 retained-frame reservation.
This changes the test caller's declared workload, not a production ceiling.
The targeted actual worker/Save/Open case passes. Failed full evidence remains
`artifacts/local-ci/20261017T141627Z-89de27ca/summary.json`.

The next full run found the same outdated caller reservation in the mixed-rate
clip/history integration fixture. It now declares441+480 original/derivative
frames and checks original-rate retention through Undo/Redo. The entire Session
package passes182 tests, zero failures or ignored tests. Both failed full runs
remain recorded; the second is
`artifacts/local-ci/20261017T142427Z-f1c8bc8d/summary.json`.


## current device-original source classification

Schema34 requires a complete source inventory in addition to tuning/Main/device
manifests. Every catalog asset has a required ordinary/null or original-content
classification, including unassigned originals. Missing inventory entries cannot
default to ordinary clip media. Superseded alpha33 refuses; explicit historical
ordinary importers remain. Source facts are projected from actual native catalogs
on save and are compared with actual reconstructed catalogs on open, separately
from constructor/lifetime admission. A device-original asset cannot be reused as
an unaccounted clip derivative. General asset-aware presets and broader device
resource/publication classes still have their own outstanding acceptance.
