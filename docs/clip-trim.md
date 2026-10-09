# Clip edge trim contract

Timeline and source positions use prepared project-rate frames. These are original
Hex policies; measured Live equivalence remains open. Schema 7 stores source phase
and explicitly migrates schemas 1–6 to zero phase.

A loop's phase identifies the first played frame relative to its bounded source
interval. It must be less than source length. One-shots require phase zero.
Rendering and waveform summaries apply the same wrapped source coordinate.
Loop splits retain the interval on both children and advance the right phase by
the left duration modulo source length. Recursive splits preserve PCM and fade
domain coordinates; source audio is shared rather than copied.

`TrimClipEdge` accepts an absolute inclusive left start or exclusive right end.
The metadata-only `preview_edge_trim` helper owns the same bounds as the command.
One-shot left trims change source offset, source length and duration together;
right trims change source length and duration together. Both retain the existing
duration-minus-source-length difference, including silent tails. Clamping keeps
at least one source and timeline frame, respects media bounds, and prevents end
overflow. Loop left trims change start/duration and advance or retreat phase;
loop right trims change duration only. Explicit source-region `TrimClip` resets
phase to zero when the region changes.

Actual trims reanchor and clamp the fade domain as documented in `clip-fades.md`.
Unchanged edge bounds preserve the complete metadata and create no undo step.
Pointer release commits one canonical command on the original track. Cancellation,
busy state, changed project/revision/media/viewport or an off-track drop creates
no edit. Musical snapping and drag autoscroll remain future work.

Source checks cover endpoints, overflow, silent tails, phase wrap/extension,
recursive split rendering, history and strict migrations. The CLI integration
test saves and reopens an original three-sample loop and checks independent PCM
values through trims, undo/redo and recursive splits. Native gesture acceptance
and performance limits are recorded in `plan.md`, `issues.md` and `performance.md`.
