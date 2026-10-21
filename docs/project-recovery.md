# Project recovery — Pulse25 / PR151

## Finite contract

The user explicitly included untitled projects, then limited untitled recovery to
**the most recent project only, with no history**. Native startup enables
`Service::with_recovery`; headless callers/tests opt in with an explicit slot.
No new project schema or second editing model is introduced.

After a committed canonical revision, the Session worker serializes a bounded
complete project checkpoint before sending its completion event. This includes
imports, edits, undo/redo, committed device gestures, relinking and explicit
recovery. In-progress gestures and drafts are not canonical and are not saved.
A checkpoint failure does not roll back an already committed musical edit; the
working project, dirty flag and compiled playback remain usable, and the event
reports the failure. Recovery covers the last successfully published checkpoint,
not every input action or a revision whose checkpoint failed.

## Ownership and discovery

Untitled projects share exactly one slot:
`$HOME/Library/Application Support/hex/recovery/untitled.hex-journal`, following
the existing native application-support convention. The worker creates the
recovery directory lazily. Missing HOME has no fallback to another user's
location. A symlink/non-directory at the immediate recovery directory is refused.
Startup of an empty project does not erase or replace a retained checkpoint.
The most recently serialized untitled checkpoint replaces the recognized slot;
there is no project-history backup, list of session directories, or automatic
retention/pruning. Multiple application instances share that slot.

Saved projects use `.<filename>.hex-journal` in the saved document's directory.
It is separate from `.<filename>.hex-recovery`, which continues to hold the
previous durable save. Saving to another directory does not change the live
model's relative-media base; checkpoint/save snapshots normalize media paths to
absolute paths before serialization. Original path/name casing is retained.

Each slot has a stable `.lock` file containing only a strict magic header and eight-byte publication
generation, never project data or history. Pinned `fs2` 0.4.3 supplies safe
control-side advisory OS file locks compatible with the workspace Rust1.85 MSRV.
This narrow native boundary belongs exclusively to storage: no unsafe Hex code,
callback I/O, process inspection, PID-based stale-lock deletion, new UI framework,
or registry-cache patch. The OS releases the lock when a process terminates;
locking is nonblocking and contention is surfaced as a checkpoint failure.
The generation distinguishes even byte-identical checkpoints from different
owners and survives clearing the slot. Untitled writers can supersede recognized
checkpoints under that lock; saved-journal writers use exact-byte ownership and
refuse competing changes. Save cleanup deletes only the exact owned generation,
so saving an older instance cannot delete a newer instance's untitled recovery.
Lock inodes are retained and never unlinked during operation.

Pulse2450 / [PR175](https://github.com/PORTALSURFER/hex/pull/175) explicitly
releases each acquired slot lock on normal success and error exits, including
metadata/magic validation failures. A private guard starts only after exclusive
acquisition succeeds; Drop supplies best-effort release during unwinding or a
checked-release failure. Closing just the owner's descriptor can otherwise leave
a Unix flock alive through a forked/duplicated open-file description. No lock
inode is unlinked, and admission remains nonblocking.

`StorageError::JournalLockRelease` retains the exact slot path, unlock error,
original operation error when present, and `JournalChange`: `Unchanged`,
`CheckpointPublished`, or `CheckpointCleared`. These describe visible checkpoint
state, not whether a saved project succeeded. Published bytes remain adopted;
cleared checkpoints remain cleared even when release fails. The Session worker
presents this as automatic recovery information rather than an uncommitted-save
or data-loss claim. A later stale-owner clear still refuses a recreated generation.

The deterministic baseline proof on clean06ccfef reproduces WouldBlock after
both successful owner close and an acquired invalid-magic error while a safe
descriptor clone remains alive; explicit unlock permits reacquisition. Production
regressions exercise the guard's same two boundaries, genuine contender refusal,
unchanged checkpoint/counter, and injected release errors before/after publication
and clear. The duplicate-description regressions are Unix-only; error ordering
and generation checks are portable. Original worker3 full-CI failure and worker4's
unlogged-return Conflict assertion remain historical evidence, not proof of
checkpoint deletion or an observed fork trace. Native macOS locking/durability
and power-loss acceptance remain separate; source/commands/digests are recorded
in Pulse and PR175 on the final exact head. No native test build is supplied.

A journal has distinct `HEXJRNL` magic, version1 bounded target-basename/payload
fields, validated native project JSON, and a nonzero publication generation.
Malformed, future, oversized, wrongly bound, symbolic-link and unknown journal
files are preserved and refused. Staging files start at0600 on Unix; complete
bytes are flushed/synced before atomic create-new or replacement publication.
Prepublication failure preserves the previous complete slot. After publication,
a directory-sync/cleanup warning retains the complete file and the writer adopts
its bytes. Ordinary success leaves one checkpoint and its generation lock, with
no journal backup. A process killed during staging can leave an unreferenced
`.hex-r-*` temporary; it is never discovered or offered as recovery, and this PR
does not delete unknown/orphan files. OS/disk failure durability remains limited
to the filesystem's sync/rename/link guarantees, not physical power-loss proof.

## Explicit recovery and missing media

The native File menu offers:

- `open and relink media…`: select a saved project, locate each missing WAV by
  stable asset ID, then prepare a complete candidate.
- `recover latest untitled project…`: read the one application slot.
- `recover saved project edits…`: select the saved project to read its journal.
- `recover previous save…`: select the saved project to read its prior-save sidecar.

These are separate explicit source choices, with **no automatic precedence** or
silent startup restore. If a journal is malformed, recovery does not silently
fall back to the previous save. A working project requires replacement
confirmation; cancellation preserves it. Metadata inspection runs off the UI
thread, while decoding/compilation and journal work remain on the Session worker.
Native picker behavior and these confirmations still need macOS acceptance.

All recovery sources open dirty and **without a saved-document overwrite
association**. Save requires Save As; selecting an existing file is refused by
the existing create-new contract. Source saved documents and recovery sidecars
are never overwritten by recovery. A recovered snapshot becomes the current
untitled checkpoint; a durable subsequent save clears only its owned slot.
Opening an ordinary saved project with relinks retains its original byte token
and document association, marks dirty, and changes only asset paths.

Relinking keeps canonical asset/track/clip/note/scene/device/Return IDs, allocator
high-water, Arrangement placement, source regions, Session cells, routing, mix,
tempo and meter. All referenced **and unreferenced** assets must decode within
existing byte/frame/resident limits, match stored source rate/frame count and
converted length, and the complete candidate must compile before adoption.
A rejected source, relink, decode or compilation preserves the working project,
history, document association, revision and prepared playback. Same rate/length
is **not** a content fingerprint: the project has no recording hash, so a
replacement with different audio but matching metadata can be accepted. The user
chooses the recording; no same-recording guarantee is claimed.

## Persistence boundary and evidence

Journal and Save/Save As reopen the canonical schema17 snapshot: Arrangement
and Session audio/MIDI/Stop cells, source bounds, stable IDs, musical maps, mix,
sends/Returns and opaque device state. They do not persist undo/redo stacks,
clipboard, window/selection state, live launched routes, transport cursor,
resident PCM or mutable processor history. Session playback after restart needs
explicit launches; this is the existing architecture's round-trip boundary.
Projects must match the active device rate and existing resident/import budgets.

Storage tests cover latest-only replacement, identical-content generation
ownership, cross-process lock contention and foreign-lock preservation, malformed/oversized/foreign/symlink preservation, injected failures,
and real child-process exit immediately before/after journal publication. Worker
integration tests cover abrupt process exit after an acknowledged mixed project,
fresh-worker recovery, automatic checkpoint failures retaining editable state,
partial media-preparation refusal, missing-media path-only recovery, saved-source
preservation, Save As to a different directory, Save Current, previous-save
recovery, and explicit Session relaunch PCM across different block partitions.
These are Linux headless checks, not native dialogs or physical output evidence.

The full exact-source local runner is required. The initial Linux run was blocked
by upstream `xattr` referencing `libc::ENOATTR`; the separately merged PR161
supplies that compatibility repair. Existing cloud sessions may still need native
library prerequisites. Required checks that fail or do not run prevent a merge;
PR151 stays draft pending native acceptance and user authorization excludes merge.

## Native/manual gate

On a disposable original macOS fixture, with no user project open:

1. Edit an untitled mixed project, terminate the process after the worker reports
   completion, restart and explicitly recover it. Start/edit a newer untitled
   project and verify only that latest project is offered. No history is offered.
2. Save As, edit, terminate and recover saved-project edits. Compare saved file
   bytes before/after recovery; Save should request a new destination.
3. Move media; use relinking for Arrangement and Session audio. Cancel midway
   and select a wrong-length replacement first; the working project must remain.
   Retry correct media, verify clip/source placement, cells, maps and device state.
4. Save As in another folder, Save Current again, reopen and explicitly launch
   Session clips/scenes. Check native dialog/focus/close guards and listen.
5. Select previous-save recovery separately; never infer it is newer than the
   journal. Keep unknown/malformed recovery files and surface the refusal.

Native compilation, these interactions/listening checks, physical power-loss,
ACL/xattr fidelity, noncooperating writer races and forced termination before
checkpoint completion remain unverified. No native or user acceptance is claimed.


## sampler original-source save/open extension

The bounded sampler path uses current schema34 and the same save tokens,
previous-save recovery and relative-media base. Save projection records actual
native catalog original content facts, including unassigned assets, without a
history edit. Open reconstructs/compares original native sources before private
snapshot preparation and old/new admission; changed/missing/cancelled or refused
candidates preserve the current document and sound. Original-content metadata is
not constructor or lifetime authority. See [the finite sampler contract](sampler.md#save-and-reopen-original-sources--pulse2906--pr499).
The normal New confirmation and Open picker/unsaved-dialog generation fences are
unchanged. This does not introduce alpha migrations or automatic media recovery.
