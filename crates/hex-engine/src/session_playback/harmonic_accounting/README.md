# Private complete-admission proof (#2342 / PR158)

This isolated `cfg(test)` module starts from frozen integration
`0f17ed6abc002346d775f3a5078a82fb4ca8441e`. It installs no constructor, codec,
publisher, registry entry, pool, callback hook or UI behavior. The source v8
version and 380-byte prefix remain unchanged. The proposed suffix is opaque
storage: it does not define amplitude, Normalize, Repeat, range or reference laws.

## Actual supported 64-bit layouts

| Proposed storage | Bytes |
| --- | ---: |
| Payload (380-byte prefix + four f32[64] banks + eight u32 fields) | 1,436 |
| Context inline / complete declared payload charge | 1,112 / 2,564 |
| One complete history, including enable-ramp state | 2,024 |
| Shared law / pair with two histories and transaction | 4,056 / 8,112 |
| Host / tail owner header | 136 / 144 |
| Captured source record / metadata / slot | 2,256 / 48 / 2,304 |
| Owner-qualified handle / store header | 24 / 104 |
| Actual reserved pool: 1,142 slots + two 256-handle maps + header | 2,643,560 |

The pool is 4,104 bytes larger than the earlier 2,639,456-byte prototype.
The context charge includes the canonical payload allocation and its two Arc
counters. Wrapper owners, their Arc counters, prepared-context Arc counters,
table header/counters and actual reserved owner/index capacities are separately
reported. A duplicate target f64[4][64] bank demonstrably exceeds the context
4,096-byte or pair 8,192-byte limit; it is not hidden behind a pointer.

The independent retained recipe budget charges 2,304 + 2,564 = 4,868 bytes per
recipe: **430** cost 2,093,240 bytes; 431 cost 2,098,108 and are refused by the
unchanged 2,097,152-byte limit. The 630 physical retained-slot bound remains,
but is not an admissible complete-context recipe count. Full owner/generation/
context/boundary/clock identity controls deduplication; conflicting charges are
refused. Roots and generation advancement are borrowed inputs, never mutated.

## Current-source qualification

The following measurements describe the frozen checkpoint above. Integration
with main `d7915f8` changes instrument accounting and runtime layout; these
positive fit claims do not apply to that combined source. See
[the current integration evidence](../../../../../docs/operator-harmonic-retarget.md#current-main-integration--15-october-2026).
The tests retain the original maximum mixed case as a current admission refusal
and separately test an admitted mixture with real track/Return/mutable owners.
No pool is installed and no budget is raised.

## Budget provenance and finite reachable inventory

`SessionPlayback::accounted_runtime_storage_bytes` in `preroll.rs` charges
playback-owned buffers and retained context handles, while `shared_analytic.rs`
and `prepared_revision.rs` independently enforce context payload budgets.
The proof preserves this existing separation and duplicate-header charging.
It also reports `complete_owned_bytes`, adding actual proposed context payloads
and wrapper/table overhead. **That combined figure is not the existing playback
budget**, and an `Ok` planner result does not claim all ownership fits 32 MiB.
Prepared revisions/assets/static process tables are not a new global memory cap.
If 32 MiB is intended to include context payloads too, the following EQ/mixed
projections fail that stronger contract; resolve it before runtime integration.

All cases use real preparation and inheritance: 256 tracks with 64 effect slots,
16 Returns with 64 slots, fresh candidate IDs, and predecessor PCM rendered for
96 frames. Neutral Utility padding keeps allocated capacity maximal. Stateful
EQ uses +6 dB and actual sends; the mixed case asserts 16 retained Return chains.
Both active and retained instrument ledgers have their real 128-entry capacity.
Legacy split/recomposition must equal the actual accounting exactly before any
proposed replacement. Remaining buffers, suppression/release banks, track/Return
chains, context lease handles and original duplicate charges stay in the base.

| Reachable shape | Existing playback | Proposed playback | Complete reported ownership |
| --- | ---: | ---: | ---: |
| 256 DC-removing Utility chains | 27,107,304 | 29,750,864 | 31,258,264 |
| 256 EQ chains | 30,728,168 | 33,371,728 | 34,879,128 |
| 256 Saturator chains | 26,886,120 | 29,529,680 | 31,037,080 |
| 256 Compressor chains | 26,918,888 | 29,562,448 | 31,069,848 |
| 256 Gate chains | 26,951,656 | 29,595,216 | 31,102,616 |
| 256 Auto Pan chains | 26,984,424 | 29,627,984 | 31,135,384 |
| 256 active + 256 retained Operator LFO pairs | 29,880,296 | 32,663,120 | 32,669,336 |
| 256 active + 256 retained Operator feedback pairs | 29,994,984 | 32,663,120 | 32,669,336 |
| 256 Sine instruments | 18,970,600 | 21,614,160 | 21,620,376 |
| 256 FM4 instruments | 18,970,600 | 21,614,160 | 21,620,376 |
| 128 EQ chains + 128 LFO pairs, active and retained | 30,304,232 | 33,017,424 | 33,774,232 |
| 192 EQ tracks + 16 EQ Returns + 48 feedback pairs, active and retained | 30,590,696 | 33,238,864 | 34,464,792 |

The EQ playback margin is only 182,704 bytes under 33,554,432. These twelve
registered configurations exercise maximum slot/count axes, not every valid
parameter, repeated edit/history, sample-rate or mixed-registration combination.
Each new candidate still requires checked postinheritance admission; no universal
fit is established. Current contexts have their own 256-count/4,096-per-context/
2-MiB limits; retained contexts have 16,384-count/4,096-per-context/2-MiB limits.
Active and retained processor declarations independently enforce 256 processors
and 2 MiB, with individual instrument pairs at most 8,192 bytes; retained suffixes
remain bounded at 4,096. All sums/capacities/generation advancement are checked.

## Proof boundaries and next contract decisions

The seven tests cover exact byte/count boundaries and +1/overflow refusal,
owner-generation roots, full-identity aliases/conflicts, actual capacities,
complete layouts, raw signed zero and inactive bank bits, and refusal without
clock/ownership/next-PCM mutation on real retained track/Return/instrument owners.
The proposed histories reserve both twins, gate/occurrence/phase/recurrence/LFO,
transition and enable-ramp fields. Gate `None` and `Some(u64::MAX)`, voice
presence, occurrence identity and inactive signed-zero recurrence remain
independently representable; no new sentinel compression is assumed. Only 80
bytes remain below the pair limit. This is storage proof, not future harmonic
DSP equivalence, migration validation or reference behavior acceptance.

Before runtime implementation, approve exact source-capture/provider format,
owner namespace lifecycle/import/retirement, cross-kind history adoption and
bounded owned-pair dispatch. The existing eight-word enable frame is not this
2,256-byte source record. Validate complete staged leases and rollback before
predecessor mutation, including generations, chains and already retained roots.
No v9 codec or control semantics may be published from this opaque proposal.
Production harmonic/painter, total/net callback allocation and performance,
native reference/settings/painting/Undo/reopen/listening and human confirmation
remain incomplete. This checkpoint stays blocked/draft under the coordination HOLD.
