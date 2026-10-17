# streaming edit, transport and fault evidence — pulse2546 / PR241

This is the source-phase acceptance ledger for the focused PR232–241 continuation
of original Pulse24 / PR177. It preserves the full original scope; it does not
turn the current audio-only/Utility constructor class into general streaming
support. Instruments, MIDI, typing and other undeclared owners remain outside
this admitted class. Default Resident behavior and its import limits remain.

Required exact-candidate full local CI is merge evidence. Native UI, listening,
physical deadlines and backend-stop results are separate evidence, **not a merge
gate**. None is claimed here. Historical planning/review holds and earlier results
remain history; the owner's 17 october instruction authorizes source merge and
continuation without waiting for manual testing. Pulse issue Done is not a native
acceptance result. Original Pulse24 remains the place for unperformed broader
acceptance; no automatic goal completion is implied.

## Original eight requirement ledger

These are the eight original criterion IDs retained in
[the source-support matrix](streaming-source-support-matrix.md#every-original-24-acceptance-criterion).
That document records their historical provenance; the dispositions below describe
this focused continuation, without rewriting the earlier record.

| Original criterion | Source evidence and boundary | Still open |
| --- | --- | --- |
| `full_local_ci` | Full `python3 scripts/local-ci.py` on the final clean unchanged PR241 candidate. Commit, content digest, command logs and report hashes are recorded with the PR, not inherited from PR177 or another stage. | Any later source change needs new exact checks. |
| `graph_catalog_leases` | Actual constructor/source-pair custody, scoped known DATA and bounded opaque RESOURCE lifetimes in [candidate publication](audio-candidate-publication.md); independent A→B→C Utility subset disposal in `actual_native_abc_moves_independent_utility_subsets_and_reclaims_each_original_bank`. | Broader provider/instrument/context classes and arbitrary caller/export aliases are not admitted by this evidence; no total RSS/OS-byte claim. |
| `callback_endpoint_retirement` | Real four-terminal endpoints, completed-handle frontier, original owner/loan retention and lifecycle cleanup. See [final-active and endpoint controls][retirement]. Application nonrenderability is distinct from OS dispose success. | Physical CPAL/CoreAudio stop/dispose behavior and native fault/shutdown exercise remain unverified. |
| `prewarm_readiness` | [Exact chunk/protection protocol](streamed-native-readiness.md), actual grouped Retry/cancel and partial-commit/pool controls; [seek/launch controls][actions] keep original due times and revoke speculative prewarm under pressure. | Native first-audible delay, buffering presentation and real device responsiveness remain unverified. |
| `native_100_track_advancing` | PR240 supplies 100 independent mixed-rate120s sources, canonical44.1/48kHz projects, hashes and an explicit real-adapter headless smoke. See [workload](streaming-100-track-workload.md). | Sustained advancing native100-track playback/listening, actual hardware duration/deadlines and counters remain unperformed. Two seconds of headless committed PCM is not this result. |
| `native_edit_seek_launch_fault` | The finite matrix below maps actual source, adapter, engine and ownership controls. This PR adds a connected mixed-rate direct-seek/Pause/priorityStop control. | Every physical/native matrix cell below remains unperformed until an attributable operator record exists. |
| `resident_import_limits` | Verified mode branches before full PCM decode; Resident caps are unchanged. Empty/resident-only successors in an opted-in generation still use normal sealed N2 custody; no automatic fallback. | Do not raise Resident caps on the strength of source merge, packaging, or headless results. Full original acceptance remains open. |
| `human_review` | The original exact-full-scope approval has not been recorded. Current owner policy permits focused exact-green source merges before optional manual review. | Do not invent human approval or characterize this ledger as original full-scope acceptance. |

## Finite source-test map

Names below are executable tests in current source. The final full runner covers
workspace tests and the explicit nondefault allocation suites; the large PR240
fixture is separately invoked/ignored by default. A source citation is a coverage
map, not a claim that every native case has been run. All rows still need the
native observations in the last column.

| Matrix cell | Existing automated evidence | Native observation still pending |
| --- | --- | --- |
| Edit / Undo / Redo / retained source identity | [Session source tests][sources]: `verified_batch_exceeds_resident_budget_without_pcm_and_undo_retains_source_epoch_and_bounds`, `invalid_verified_batch_is_atomic_and_metadata_reservation_precedes_canonical_adoption`. [Connected adapter][adapter]: `canonical_service_installs_verified_mixed_rate_and_empty_successors_despite_lost_submitted` checks actual installed source identity, unsupported-edit refusal, Undo and empty successors. | Repeated live edits/Undo/Redo while100 tracks advance; audible continuity, responsiveness and truthful last-applied versus edited state. |
| Save As / reopen / new generation | [Session source tests][sources]: `canonical_verified_bindings_survive_save_as_and_changed_bytes_require_explicit_reopen`. [Connected adapter][adapter] checks old Client delaying the actual replacement lease, stale RestartAudio refusal and a new output identity. | Save/reopen the disposable workload, no source-path confusion; actual UI close/relaunch and no loss of edits. |
| Same catalog / N2 pressure / refusal / retry | [Connected adapter][adapter] checks same actual catalog reuse, source-free N2 occupancy and sequential imports that retire the unused empty catalog. `exact_root_domain_retirement_intent_survives_cleaning_and_recovers_before_reuse` checks exact report consumption/incarnation reuse. [Readiness tests][readiness]: `ready_candidate_group_retry_releases_speculation_for_old_page_and_remains_cancellable`, `two_source_small_pool_refusal_after_commit_preserves_pcm_clock_and_applied_prefix`. | Busy/refused status and retry under actual editing; no silent cap widening. Exact N2 counters/forced pool pressure are not exposed as native UI controls and remain headless evidence only. |
| Seek / Pause / priorityStop | New [connected adapter][adapter] `verified_adapter_seek_pause_and_priority_stop_preserve_applied_source_and_domain`:24kHz file→48kHz project, actual applied identity/domain, first-region PCM, direct seek into a distinct source window, exact position/resolved prefix, Pause silence/held clock and priorityStop with a full32-command ring. Every real callback entry uses total and signed-net allocation probes. [Action tests][actions] additionally exercise blocked seek, Busy, Pause cancellation and PlayFrom. | Ruler seeks during native playback, Pause/Play and Stop responsiveness, audible discontinuities and UI ACK labels. Headless probes do not measure native API allocations or deadlines. |
| Loop / quantized Session routes | [Engine action tests][engine-actions] `quantized_action_projects_original_due_across_real_loop_wrap_then_matches_activation`; [Session tests][session-tests] `session_audio_quantized_launch_begins_on_the_due_performance_frame`, `session_audio_seek_pause_stop_scene_stop_and_return_are_explicit_and_keep_midi_routes`. [Action tests][actions] `original_quantized_due_survives_pause_cancel_and_revocable_prewarm_pool_pressure` verifies actual worker pages, missing-page hold exactly at due, original due retention and successor observation. | Loop toggle/clip-loop range, scene/cell launch, stop selected/all and back to Arrangement with native audio. These typed contracts are supported; this PR adds no claim that they have been manually exercised. |
| Missing / changed media | [Session source tests][sources] malformed-batch atomic refusal; [connected adapter][adapter] changes bytes after canonical fingerprint capture and checks actual worker refusal without replacing the applied source. [Readiness tests][readiness] `changed_file_pending_domain_refuses_without_loan_or_ack_and_keeps_pool_until_report`. | Missing file before open, truncation/metadata mismatch and changed bytes after admission on an independent fixture. Same-size PCM edits **before** open may be legitimately recaptured: saved projects do not persist the verified fingerprint. Renaming an already-open file may leave its descriptor readable. |
| Worker / cleanup / device failure | [Pending worker tests][pending] `insufficient_allowance_refuses_before_constructor_allocation_and_actual_worker_panic_is_retained`; [disposal tests][disposal] `actual_factory_worker_panic_payload_is_retained_without_drop_under_worker_mutex`. [Connected adapter][adapter] `failed_output_setup_keeps_canonical_editing_and_saving_available` preserves editing/saving and the temporary recovery journal. | Device loss, physical stop failure and worker-fault presentation are unverified. No native fault-injection UI exists; do not claim these headless faults reproduce CPAL/CoreAudio failure. Incomplete ownership remains retained, not a successful in-process restart. |
| Stop / close / abandonment / eventual reuse | [Candidate controls][candidate-tests] `live_control_abandonment_finishes_busy_multiphase_cleanup_without_another_ui_poll`; [final-active tests][retirement] `exposed_missing_handle_cannot_return_an_actual_active_owner_even_after_both_guards_release`, `carrier_destructor_panic_after_actual_extraction_keeps_original_owner_and_never_publishes_ready`. [Connected adapter][adapter] drops Service/Client/backend then reopens after autonomous actual lease release without UI reclaim calls. | Actual close/reopen, user-visible Stop under stress and backend terminal behavior. Normal cleanup evidence must never be substituted for failed/unknown physical stop. |

## Optimized operator checklist and result record

Use the repository's supported stable optimized delivery, exact full-CI report,
included PR heads and [PR240 workload](streaming-100-track-workload.md). Use the
installed receipt's source; do not relabel an older installed app after merge.
No agent GUI interaction or force quit. Save and quit before disk replacement;
installation does not reload an already-running process.

1. Record source/executable/receipt and project/manifest hashes, machine/OS,
   device, project/observed device rate, duration and actual callback size if
   observed. Otherwise record unknown. Start with the44.1kHz document for a
   44.1kHz device and explicitly choose “open as verified streaming…”.
2. Record the PR240 baseline separately: actual current applied revision,
   transport progress, sustained audible output60–90s and starting/ending
   callback maximumµs/miss counter/render-fault values. Initial buffering and
   later dropouts are distinct observations.
3. While playing, make a bounded clip/gain edit, Undo/Redo, seek at several ruler
   positions, Pause/Play, toggle a selected clip loop (`cmd-l`, then `l`), and
   Stop. For every action record edited/current applied revision, resolved
   command behavior, output/clock continuity, buffering/refusal and responsiveness.
4. Launch a cell/scene at the existing4-quarter-note quantization, then stop the
   selected/all Session clips and return to Arrangement. Record scheduled and
   activated separately; do not call scheduled ACK audible activation.
5. Save As to a new disposable path, close normally, reopen, and verify sources
   and edits. Keep the original workload intact. For missing/truncated media,
   first make a separate independent fixture directory (relative media paths);
   never alter user assets or a shared baseline. Only an after-verification byte
   mutation tests the retained-fingerprint boundary; record its timing explicitly.
6. Record ordinary Stop/close/reopen results and failures. Cases without a safe
   operator control (exact pool pressure, worker panic, CPAL stop failure) stay
   **native unverified / headless evidence only**. Do not manufacture faults,
   claim OS disposal or add a new fault/recovery framework.

For each row retain: `not performed / pass / fail / unknown`, exact action and
elapsed duration, expected versus actual clock/route/PCM/listening result,
starting/ending applied revision and diagnostic counters, and any saved error.
Native page/cache occupancy is currently unknown. A transient status observation
is not ownership authority. Keep all failures and the explicit remaining original
Pulse24/provider scope visible even after source merge and stable installation.

[adapter]: ../crates/hex-desktop/src/native_audio/tests.rs
[sources]: ../crates/hex-session/src/service/verified_source_tests.rs
[actions]: ../crates/hex-audio/src/prepared/eligible_publication/tests/actions.rs
[readiness]: ../crates/hex-audio/src/prepared/eligible_publication/tests/readiness.rs
[retirement]: ../crates/hex-audio/src/prepared/final_active_tests.rs
[candidate-tests]: ../crates/hex-audio/src/prepared/candidate_tests.rs
[pending]: ../crates/hex-stream/src/runtime/pending.rs
[disposal]: ../crates/hex-stream/src/runtime/disposal.rs
[engine-actions]: ../crates/hex-engine/src/session_playback/stream_action_tests.rs
[session-tests]: ../crates/hex-engine/src/session_playback/tests.rs
