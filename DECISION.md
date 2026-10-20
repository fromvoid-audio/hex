# actual compiled owner and pre-commit constructor boundary

Read-only implementation decision return; NOT a source-ready component, admission, or full Service qualification. Canonical PR395 stays b94b0e2f6fbec0932d422c00d4584739c192cb9a. No Project/history API was edited.

## actual runtime and budgets

The desired real Service/WorkerPublication closed-constructor test remains RED. Session61095 failed UnsupportedGraph in the LEGACY per-project quote before construction. That legacy MIDI/instrument refusal is unchanged. The distinct existing for_supported_limits(0) original-class ceiling then reaches the closed normal walk. Selected retained/peak2251030528; unchanged production Root retained7397769794/peak7589999170; quote metadata2217476096. These are configured conservative bounds, not physical allocations/RSS. No shared-host-versus-selected cap mismatch is established.

Session64738: all mandatory schema descendants reached normal shared closure; 80-byte canonical-midi-body:0 has strong3, declared2, weak0. Session97371 borrow-only field comparison diagnostic directly identifies the third actual field: WorkerDispatchState.compiled -> PreparedRevision.mutable_track_revisions[*].arrangement_midi_sources[*].source. It shares the actual target source allocation but is a distinct owning field (shares_target=true, same_field=false, strong3, weak0). PreparedRevision's existing constructor clones clip.source_identity(). The public selected-source visitor can observe this field but cannot qualify its enclosing legacy revision/Playback; it was NOT imported to make admission green.

The corrected finite schema now carries actual legacy compiled and Event.playback field borrows and refuses these unsupported owning parents BEFORE descendant traversal, including EMPTY revisions. This prevents accidental empty-root admission; it is NOT the intended end state. New Session24557: one affected test passes, including independent real empty PreparedRevision and actual Playback refusal; ledger stays invalid on refusal. All temporary trace code is removed, including the temporary diagnostic method. audio_storage.rs is byte-identical to its pretrace WIP. Trace input bytes/raw are preserved separately.

## existing exact signatures and order

1. Session::command_prepared_revision_with<T>(&mut self, command: Command, previous: Option<&PreparedRevision>, finish: impl FnOnce(&Project,&Project,&Arc<PreparedRevision>) -> Result<T,SessionError>) -> Result<(Created,(Arc<PreparedRevision>,T)),SessionError> (lib.rs646). Existing code prepares the raw revision and Arc::new BEFORE finish. Undo/Redo follow the same order. Reusing finish would retrospectively adopt an already-born legacy product, which is forbidden.
2. Session::original_sampler_session_loan_on_control(&self) -> Result<OriginalSamplerSessionLoan<'_>,AudioStorageError>. The loan stores session:&Session plus actual waveform RefMut and transform writer guards. Its resident projection holds &Session; Engine's mandatory resident visitor observes that ACTUAL enclosing body and checks containment of editor, transforms, sine, cancellation, media and waveform fields.
3. Editor::execute_prepared_transition<T,E>(&mut self, command, prepare: impl FnOnce(&Project,&Project)->Result<T,E>). history.rs82 clones/applies/validates candidate and reserves undo backing BEFORE callback. Callback sees source/candidate, NOT the actual Editor/history. Only after successful preparation does it move candidate/current/history. No allocating history growth follows callback.
4. Engine's closed original driver consumes the whole current Editor/history plus distinct captured target under the SAME Session writers and SAME Audio guards. It does not expose a plan or quote.

Two actual rustc borrow probes in this packet both produce E0502: holding the existing loan across &mut Session transition, and trying to acquire that loan inside the existing transition finish callback. These are type-boundary failures, not admission failures. Diagnostic comparisons clone no owner; diagnostic printing was not an allocator probe, and no total-zero-allocation result is claimed for that traced run. The latter hook also runs too late (legacy birth already occurred).

## minimal owning-contract choice needed before changing public model API

Recommend a purpose-specific staged-transition extension INSIDE the existing Editor owner (no new registry/worker/runtime owner): retain the actual pending candidate and its phase in Editor while it is being prepared; expose only an immutable current/history/pending candidate observation; after native construction succeeds and readonly ledgers/loans end, commit that SAME actual pending candidate using the already-reserved history destination. Refusal/cancel clears only through actual pending cleanup; a consumed/panicking cleanup must remain failed and nonretryable. Ordinary APIs/behavior remain supported.

Proposed operations (names provisional; no implementation yet):
- begin_original_sampler_transition_on_control(&mut self, Command) -> Result<Created,Invalid>, storing actual pending Project/Created in this SAME Editor;
- pending_original_sampler_project_on_control(&self) -> Result<&Project,Invalid>;
- commit_original_sampler_transition_on_control(&mut self) / cancel_original_sampler_transition_on_control(&mut self), reachable only from the audited Session closed command operation, not a caller-supplied approval token.

The owning Session operation can then safely inspect &Session (including the ACTUAL current/history/pending candidate) while its existing writers stay held, call the private Audio closed driver, end all immutable ledger/projection loans, and then reborrow the SAME Editor to commit. The actual new captured target SharedProject/source, old/new candidate/history backings, command-owned state and all pending metadata need BEFORE-BIRTH preflight; the initial candidate clone/undo reserve cannot be retrospectively blessed. Pending Project traversal and changed Editor/Session inline layout must be included in both normal/bootstrap and both caps. A scalar identity, BodyRef over a copied frame, unlocked/revalidated quote or omitted current/history is not an alternative.

A seemingly smaller callback(&Editor,&candidate) change alone does NOT solve the current whole-Session resident body borrow: an active mutable Editor method prevents borrowing its enclosing Session through the existing &Session-based loan, and copying body addresses would violate the actual-reference contract. Splitting Session fields would additionally need an explicit enclosing-layout/containment contract; do not silently weaken BodyRef or invent that protocol.

This is a concrete public Project/Session transition ownership-state decision beyond the approved immutable-current Session lending entry. Please select the staged existing-Editor candidate route or a concrete bounded alternative before those new model/API hunks. Native/default startup, queued/Event/UI reader conversion, full ACTIVE/second-parent union and actual Sampler PCM/ACK/replace/retire/export/seek/mixed remain OPEN; this is not permission to permanently disable ordinary edits or qualify legacy products. No ceiling, ledger, native publication, unsafe/Sync or owner-family widening has been applied.

## current receipts and limitations

- Session24557:1PASS0failed0ignored,17.89s compile/.01s runtime.
- Strict92506: Engine+Audio+Session all-target allocation-probe terminal0,22.49s; two existing fail-closed build-script Unknown warnings remain explicit.
- Policy35 passed BEFORE adding exact Desktop test reader, architecture failed at that actual new test consumer; preserved as such. Narrow exact test-only reader correction: policy36PASS and architecture25 crates terminal0 (39799). Foreign producer/impl/ordinary Desktop/UI controls still reject. No Rust source changed between strict and policy-only correction.
- Desktop all-target Main File advance/run_loan dead-code errors remain OPEN; not covered by the above strict receipt. No platform/native app/build dispatch or playable/full395 claim.
- All owned jobs are terminal. Source is preserved uncommitted in the same checkout. Current.diff compares the exact saved intake-stage before bytes, not an invented uniform accepted-tree baseline; SOURCE/CUMULATIVE identify actual bytes and prior hashes individually.
