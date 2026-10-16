# offline export

Pulse30 / PR152 consumes canonical capture prerequisite PR159 at frozen source
`3f1ef2279696de0acb7dec2941dfdbe5fe53c6e2` (including PR162). Engine ownership
and transactional capture semantics remain in [output capture](output-capture.md).
This is linear Arrangement export of an immutable compiled revision, not live
Session/transport recording or a capture of the currently sounding generation.
Source-loop and streamed-source refusals remain explicit engine restrictions.

## range and output

Native export uses the normalized canonical time selection's half-open frame
range; without a time range it exports the captured Arrangement audible horizon.
The output prompt explicitly chooses Main, selected tracks, shared Returns, or
selected track stems plus shared Returns. A time highlight alone never changes
Main to a selected-track output. Scope IDs/range are chosen when opening the
dialog; the actor captures the current compiled revision after the device edit
barrier. Stale or unknown IDs are errors, never reassigned array indices.

Main retains master processing. Selected-track and Return mixes use post-mix
capture and exclude Main/master processing. Stems retain each source's actual
processing, effective mute/solo, level/pan and tails. Shared Returns are separate
outputs from the actual contributor graph, including unselected send contributors;
they are not independently rerendered muted clones or copied into each track.
There is no claim that stems re-sum to a nonlinearly processed Main output.

Every WAV is stereo PCM16 at the project sample rate, using existing rounding and
clipping. Start/end are exact frames, not rounded to render blocks. Stateful
destinations replay canonical history from zero without Seek, including held
and released instrument/effect histories. Replay retains the existing 8,388,608
frame ceiling; larger stateful starts are explicitly refused. Destinations whose
canonical contract permits direct seek skip replay. Loop metadata never wraps
the linear export. Output length is exactly the selected span; no extra tail is
appended beyond its end. No alternate format, dither, normalization or general
plugin-tail support is added.

## progress and cancellation

Each admitted job owns a fresh `ExportControl`. Progress counts actual rendered
frames, including discarded preroll when required. The bounded control worker
checks cancellation before construction, between blocks of at most 256 frames,
after replay and staging, and before publication. Clicking export while a job
is pending cancels it; close/discard requests cancellation without joining.
Completion remains separate from canonical state/history and keeps the captured
revision. A cancelled reservation is released when its result is consumed.

Publication and cancellation have one atomic control-state boundary. Cancellation
accepted before that boundary prevents publication. Once publication starts,
cancellation returns false and completion reports the actual publication result;
it does not pretend to undo an already committed output.

## publication and overwrite

Create-new is the default and checks collisions at the actual hard-link or
exclusive-rename publication point. Native export starts in the user's home
folder and uses the native save dialog's concrete-path overwrite confirmation;
there is no second application confirmation. The accepted path is preserved
exactly, without appending an extension after consent. An absent destination
still publishes without replacement, including a late collision. Read-only and
permission errors explain choosing a writable folder rather than temporary-file
implementation details. No UI-thread filesystem inspection is performed.
The worker records and revalidates the concrete destination's regular-file
identity, length and modification time before atomic exchange. Links and
directories are refused at consent inspection.

Successful confirmed replacement is destructive: after atomic exchange the old
file is removed. No routine backup or recovery notification is created. Rendering
and cancellation before publication still preserve the old destination. An external writer
can race the last validation: after exchange, a changed displaced identity is a
`PublicationConflict`, naming both paths and retaining both objects. The new
export may then be visible at the destination; this is explicitly an error,
not success. No blind rollback can overwrite a later writer. Metadata validation
is not byte-level compare-and-swap; undetected in-place writes may be removed as
part of the consented destructive overwrite. Existing
open descriptors can still mutate their inode; no global writer lock is claimed.

Stems publish only as one **new batch directory**. Files use deterministic
track-then-Return stable-ID ordering and readable names such as
`track-12-Kick.wav` and `return-3-Room.wav`. Unsafe filename characters are
replaced and labels bounded to 64 UTF-8 bytes. Duplicate IDs are refused; equal
names on different IDs remain distinct. All complete synchronized WAVs remain
in task-owned staging until a no-replace directory rename. Existing directories,
including empty ones, and symlinks are never replaced or merged. Cancellation
and prepublication failure remove only recorded task-owned staging files, never
recursively erase unexpected data. Directory synchronization failure after
publication reports the visible output as not durable rather than false success.

The narrow dependency is pinned `rustix 1.1.5` (`fs` feature), licensed
Apache-2.0 with LLVM exception / Apache-2.0 / MIT. It wraps Linux `renameat2` and
macOS `renameatx_np` no-replace/exchange calls through safe Rust APIs. Workspace
unsafe prohibition remains unchanged. Other platforms, kernels or filesystems
without the operation return explicit errors; no unsafe fallback or portability
claim is made. macOS execution remains to be validated separately.

## cli

```sh
hex-cli export-project project.hex mix.wav 12000 24000
hex-cli export-project project.hex tracks.wav 12000 24000 --tracks=1,2
hex-cli export-project project.hex returns.wav 12000 24000 --returns=3
hex-cli export-project project.hex stems 12000 24000 --stems=1,2
hex-cli export-project project.hex mix.wav 12000 24000 --overwrite
```

IDs come from the canonical project. End must exceed start. Progress is emitted
on stderr; type `cancel` followed by enter on stdin to cancel. `--overwrite`
is explicit intent for that concrete existing regular file, with the same
destructive replacement semantics. Batch directory overwrite is refused. Existing
`render-project` behavior remains available.

## acceptance evidence and gaps

Focused fixtures verify independent post-fader track and effectful Return PCM,
one-pass batch stems, equal-name separation, held and release feedback range
bytes against uninterrupted export, preroll progress/cancellation and budget
refusal, cancelled reservation reuse, staging cancellation/cleanup, late file
and batch collisions, cancellation preserving prior data, successful replacement
without backups, and exceptional displaced external-write recovery.
CLI integration covers exact range PCM, stems, default collision refusal,
explicit destructive overwrite and malformed ranges. GPUI support tests cover scope
and path cancellation/reopening, the user-folder default, native-save admission
without a second application confirmation, and pending-job
cancellation. These are automated source fixtures, not native user acceptance.

Required local CI remains tied to the exact final commit and unchanged digest;
record its actual result in Pulse/todo rather than infer it from focused checks.
The user tested candidate958d1f2 on11October: Documents export works and the Main
file looks good; root-volume read-only refusal is accepted. The user requested
the simpler path/overwrite behavior above. Changed behavior requires a fresh
exact-source candidate and explicit user correctness confirmation before Done.
The latest shared pipeline is required checks, merge, configured post-merge
action/testable optimized build, explicit user approval, then Done. Failed or
stale checks still block merge; merge alone does not complete this ticket.
Pulse2390 owns the common workflow correction; its enforcement is not claimed here.
Return/stem listening is deferred in Pulse2388 until representative effect devices
and projects exist; realistic sustained long export is deferred in Pulse2389.
Finder audio drops2385, disappearing selection highlight2386 and multiple-track
selection2387 are separate work, not fixes included in this export PR. Stems
continue to use explicitly selected tracks (including empty ones) plus separately
named shared Returns; unselected tracks are not implicitly added. Native
checklist: export a held/releasing feedback range and compare timing by listening;
exercise scope/cancel/reopen; export selected stems with a shared effectful Return;
confirm batch collision refusal and one native overwrite confirmation with no
routine backup; check writable default/read-only error; close during replay;
measure a stated long-project sustained workload separately.
