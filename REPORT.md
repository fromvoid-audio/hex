# f208 retirement completeness investigation — proposal only

Source remains f20804ccc24283ade79dcff2e5bb8a11b04568ac, tree ec26909cbbfd24fdde179d12838029ddee21fc75, clean. No production or repository test edits. No Service, registration, native, application or permission operations.

## Actual first refusal

`audio_storage.rs:61–63` returns `RetirementReceiptUnknown` unconditionally after `require_known_storage`. It does not inspect a particular owner, lifecycle route or root phase. The public Copy/Default summary has public scalar fields; it is not an ownership receipt. Making this method succeed based on zero counters would accept fabricated public scalar summaries.

`sampler_compile.rs:191–204` repeats actual mandatory visitors, validates selected handle closure and reads `summary`; it does NOT call `require_complete`. The assertion at `sampler_compile/tests.rs:153` is the independent normal-inventory oracle preceding actual compilation. There is no hidden runtime require_complete failure to blame on f208 retirement. The frozen actual compiler fixture succeeds while that oracle deliberately refuses completeness.

## Actual reproduction, not source-only inference

1. Reused the existing owner-qualified f208 test artifact (SHA256 04c26d0979c0cc188ac9fce9379e74a87682f3488ce544f87fa6883d2a05de61), exact `actual_aggregate_exact_and_minus_one_limits_refuse_before_inspection_or_owner_birth`. Actual 1 PASS / 0 ignored / 754 filtered. Its real closed operation accepts the exact boundary; minus-one cases refuse before allocations/IDs/paired births/clones; normal oracle retains the require_complete refusal. No new test source/compile for this run, no relabeled full receipt.
2. Built the same frozen Engine library, then compiled a disposable /tmp Rust public-API diagnostic against its rlib. No repository source changes. Actual assertion discriminators:
   - Public default summary: all counters zero, complete => RetirementReceiptUnknown.
   - Actual empty normal ledger: zero DATA allocations, zero handles, zero roots/retirements, unknown_storage0; ledger/retained/peak35224B, complete => RetirementReceiptUnknown.
   - Actual first borrowed `AudioPreparationCustody`, role Current, phase Vacant: unknown_storage0; handles0; retirement roots1; DATA10088B (actual inline body), retained/peak45312B; selected handle closure PASS; complete => RetirementReceiptUnknown.
   - Duplicate actual-root visitor => SourceMismatch; failed ledger discarded immediately. No reuse of partial observations, totals or completeness after caught visitor failure.

The four assertions in this standalone diagnostic passed. They are not four Cargo/native tests or an admission proof. The production-layout probe is separate from cfg(test) fixture costs.

## Exact owner/field/phase gap

`Inputs::visit` first calls `borrow_custody_on_control(self.preparation, Current)` at sampler_compile.rs86. That actual typed object is the first root. The real constructor initializes root identity, incarnation0, phaseVacant, no bank/original_resources/slots and default child assemblies. The relevant ledger field is `roots[0]`: `Root { owner, _role }` at audio_storage.rs204–207. No per-root retirement binding or lifecycle evidence is stored or queried there. `summary` reports `unresolved_retirements = roots.len()` and `unresolved_handles = handles.len()` at414–415 regardless of successful physical handle closure or local retirement tests.

Thus the first selected row is an unresolved borrowed parent-root obligation, not a specific outstanding PCM destructor. In the observed first-root case there are no heap DATA allocations or shared handles to retire. Its scalar inline-header observation is not proof of a live-parent retirement protocol. The global unconditional summary refusal also occurs with no root at all. Full root/field ordering can only be enumerated after a private bound validation exists; claiming one missing Sampler table receipt caused this public refusal would be incorrect.

## Minimal next-target proposal, BEFORE any production patch

Keep public `AudioRootStorageSummary::require_complete` fail-closed. Do not zero roots/handles, whitelist roles, upgrade declarations, use strong-count equality as retirement, or infer receipts from f208's DropProbe tests.

A smallest useful first target is a PRIVATE actual-bound root retirement inspection, using the existing `Root.owner` lifetime-bound Any reference to recover the actual known `AudioPreparationCustody` (no new owner/lease/receipt pool, public permit, lifetime cast, copied totals or generic callback observer). Add a narrow side-effect-free existing-custody query in audio_preparation.rs and invoke it from a private normal-ledger validator in audio_storage.rs after successful mandatory visits/closure. Inspect ACTUAL phase, root identity/incarnation where relevant, bank/resource/slot fields, child discovery, pending/tombstone/first-panic states and backing ownership. Exhaustive typed field inspection must not drift from visitors. No cleanup/drop/allocation/identity issuance during inspection. Normal mode only; arithmetic must never return this authority.

First supported discriminator can be the actual structurally empty parent from try_new, while all live/unbound classes remain explicitly incomplete. PhaseVacant alone is UNSOUND: f208 permits context-only/partial child ownership while parent/Arrangement is Vacant. RetirementPending with Option already None must still refuse. A foreign custodian with equal phase/counters cannot establish another root's ownership route. A failed visitor ledger must be dropped and never used by this checker.

This query is a proposed first missing binding, not a proposal to make the whole selected summary complete. The actual other roots (target Arrangement, Editor/history, standalone bank, previous contexts, transform/cancellation and imported original records) still need THEIR real existing custody/lifetime bindings before any private whole-operation complete result. In particular caller/editor/history ownership and future captured old Project/new authoring owners cannot be inferred from role labels; Service/worker incarnation was deliberately excluded from current source scope. If their real root ownership cannot be established from actual existing borrowed input, report the precise boundary rather than accepting a partial frontier.

Proposed acceptance discriminators:
- actual empty parent versus Vacant parent holding a real root context or partial child;
- same actual current root versus unrelated equal-looking parent;
- normal completion versus pending-after-take, failed parent/first panic and stale incarnation;
- exact physical field/alias closure preserved; omitted aliases still fail before retirement checking;
- public zero/fabricated summaries and unknown root classes never acquire authority;
- all failures discard partial ledger; no declaration/ID/allocator/owner mutation by inspection.

No production patch proposed to merely change line63 to Ok. Primary should choose/approve the actual-bound private target before source work; broad Service/publication/frontier expansion remains outside this step.
