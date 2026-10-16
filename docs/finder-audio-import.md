# Finder audio import — pulse2385 / pr172

GPUI's Arrangement `ExternalPaths` drop callback owns the supplied paths and
captures the painted stable track/insertion anchor, snapped frame and current
project generation/revision. It submits one bounded batch request. UI and audio
callback threads never read files, decode, resample or compile this import.

Payload order is preserved, with no filename sorting, first-file fallback or
deduplication. On a regular lane, full-length converted clips concatenate from
the drop frame using the existing audio/MIDI overwrite laws. At a gap, empty or
trailing Arrangement position, one hybrid track per file is inserted in payload
order at the captured anchor; all start at the same frame. A one-file drop keeps
the previous placement behavior. Main/Return rows are not media destinations.
Stale painted geometry, opening/closed/busy state, pending numeric drafts and
active/unfinished device gestures refuse the drop; finish edits and drop again.

The canonical worker captures actual Session capacities, decodes each original
WAV path sequentially, converts to the project rate and prepares cached peaks.
Existing formats/settings remain unchanged: 64 MiB/file, 8 million source frames
per file, and at most 8 million retained project-rate frames across the session,
including media retained for Undo. Canonical asset/track/clip limits, exact
overwrite fragment counts and checked timeline/accounting arithmetic still
apply. Unsupported, missing, malformed or over-budget members refuse the whole
batch. No source file is renamed, copied or written. Paths retain their spelling;
relative paths use existing lexical absolute resolution. External files are not
immutable leased snapshots and must remain available for future project reopen.

All prepared members feed one private candidate, one graph preparation and one
history commit. No partial media/project event is published. Failure preserves
project, IDs, history, dirty state, resident accounting and current playback.
Successful Undo/Redo covers the whole batch without reopening source files.
The existing shared playback-source finalization preserves target track/device
identities, prepared revision/preview identity and ordinary project-replacement
semantics. As with other graph replacements, publishing the imported graph
cancels the old computer keyboard audition lease. Release and press a key again
to rearm; additive import does not create a new keyboard target.

The status bar displays bounded phase/member progress and a `cancel import`
control; Escape also cancels a pending import. Cancellation uses a metadata-only
atomic handle, not a worker FIFO command. Decode/resample internals are not
interruptible: cancellation is observed at phase/member boundaries and immediately
before commit, with no latency promise. Cancellation before the atomic commit
claim publishes no import; afterward it reports too late and the single Undo
remains available. Window teardown or opening another project requests cancellation.
The final correlated worker event reports the actual result. A later journal or
native-publication error cannot undo an already committed model; it is reported
separately. Progress is not evidence of native audio admission.

Portable evidence includes real worker/playback and rendered UI-to-worker tests
for ordered batches, refusal, cancellation and Undo/Redo. GPUI's test platform
cannot establish actual Finder delivery, OS permissions, native drawing or
hardware audio deadlines; its external-path constructor is private, so cloud
rendered tests invoke the real drop adapter at painted window geometry.

Native acceptance remains pending on an exact optimized macOS candidate: verify
one/multiple supported files on a lane/gap/empty/trailing area, payload order and
snapping/modifiers, mixed-rate duration/source paths, unsupported middle/end and
missing-file refusal, progress/cancel/too-late behavior, prior clips/keyboard lease cancellation,
whole Undo/Redo, source-byte preservation and save/reopen. No native acceptance,
whole-ticket review readiness, merge or release is claimed by Linux checks.
