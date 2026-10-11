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

## publication and recovery

Create-new is the default and checks collisions at the actual hard-link or
exclusive-rename publication point. Native export presents a separate prompt for
the concrete destination; replacing a regular file requires the explicit
`replace this file` choice. No UI-thread filesystem inspection is performed.
The worker records and revalidates the concrete destination's regular-file
identity, length and modification time before atomic exchange. Links and
directories are refused at consent inspection.

Replacement always retains the displaced object at the reported recovery path,
even on successful overwrite. It never truncates the old file. An external writer
can race the last validation: after exchange, a changed displaced identity is a
`PublicationConflict`, naming both paths and retaining both objects. The new
export may then be visible at the destination; this is explicitly an error,
not success. No blind rollback can overwrite a later writer. Metadata validation
is not byte-level compare-and-swap; retaining the displaced object protects data
even when an external in-place write is not distinguished by metadata. Existing
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
recovery semantics. Batch directory overwrite is refused. Existing
`render-project` behavior remains available.

## acceptance evidence and gaps

Focused fixtures verify independent post-fader track and effectful Return PCM,
one-pass batch stems, equal-name separation, held and release feedback range
bytes against uninterrupted export, preroll progress/cancellation and budget
refusal, cancelled reservation reuse, staging cancellation/cleanup, late file
and batch collisions, retained prior data and displaced external-write recovery.
CLI integration covers exact range PCM, stems, default collision refusal,
explicit overwrite/recovery and malformed ranges. GPUI support tests cover scope
and path cancellation/reopening, concrete consent cancellation and pending-job
cancellation. These are automated source fixtures, not native user acceptance.

Required local CI remains tied to the exact final commit and unchanged digest;
record its actual result in Pulse/todo rather than infer it from focused checks.
Listening quality, native macOS path/consent interaction and cancellation,
physical device behavior and sustained throughput remain unverified. Native
checklist: export a held/releasing feedback range and compare timing by listening;
exercise scope/cancel/reopen; export selected stems with a shared effectful Return;
confirm collision refusal and explicit replacement/recovery; close during replay;
measure a stated long-project sustained workload separately.
