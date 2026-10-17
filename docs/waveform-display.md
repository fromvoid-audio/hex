# detailed Arrangement waveform display

## stable resize sampling — #2764 / PR372

Arrangement audio clips, ordinary trim previews and lower fade resize previews
sample a shared viewport lattice. Its timeline interval, lane origin, density
and 4,096-column cap stay fixed while clip edges move. Only the intersecting
columns and a neighbor on each side are prepared; the existing clip mask crops
the paths. Cache accounting charges the actual prepared columns. Peak hierarchy
queries and source/loop/prepared-root mappings are unchanged. Fades remain a
separate live envelope. This changes no project, Undo, worker or audio behavior.
Other fitted waveform consumers retain their own view; browser UX is separate.

The former per-clip fit divided each changing visible interval into
`ceil(width × scale)` columns. Both queried frame ranges and horizontal peak
positions changed across otherwise untouched audio during fractional resizes.
The deterministic stereo-outline regression fails with that sampling restored
and passes on a shared lattice at 1×, 2× and the column cap, for both clip edges,
newly revealed audio and retained prepared-root crops. Canonical one-shot/loop
trims and lower fade resize geometry also preserve interior outline points.
These compare the actual points supplied to GPUI's path builder, not screenshots
or display timing. Existing rendered trim and fade gesture tests cover input,
preview and release/Undo behavior.

The user-running source label is `0f99bfbc`; no resize was captured. Native
visual reproduction and acceptance remain pending in the coordinator's one Mac
lane. Without stopping/replacing the running app, record its build and zoom,
then check both trim edges and both lower fade handles at fixed zoom, with
Command held/released, through offscreen clips, loop boundaries and prepared
warp clips. Unchanged interior landmarks should remain fixed; cropped/revealed
audio and fade overlays should update, with one Undo/Redo per release. The
launcher owns the later combined optimized delivery. No subjective smoothness,
GPU timing or app-delivery result is claimed by cloud geometry tests.

## waveform visibility during trim — pulse2782 / PR388

During an ordinary edge trim, Arrangement hides the original clip's waveform
and paints a live overlay. The overlay previously always selected original asset
peaks and the raw clip mapping. That mapping deliberately refuses transformed
audio, so accepted processed samples became flat during dragging and returned
when the overlay ended. Warp-off clips with pitch/processing metadata also use
this path; untouched clips remain a useful control. The user's observed greying
only during motion is consistent with this path. Native reproduction on their
specific samples remains separate evidence.

The overlay now supplies original and prospective clips to a shared preview
helper. A retained-root crop reuses the same immutable prepared peak Arc with
its prospective crop offset from the model's tempo projection. It checks retained
root identity, projected origin/extent, original cached offset/length and new
source bounds. Missing, changed or mismatched roots are refused without clearing
accepted peaks or queuing decode, DSP or project work. Untouched clips preserve
their existing raw source/loop mapping. Project edits, timing and history remain
owned by the existing canonical trim command.

Rendered mouse regressions inspect actual path-builder ordinates, accepted peak
identity and projected crop offsets for both edges, partial sources, repeated
motion, stereo bipolar/sparse summaries at 48 kHz and 44.1 kHz, processed Warp-off
clips and Beats-warped clips. The old path produced zero-height stereo paths in
all 24 processed motion cases while untouched controls remained visible. These
are deterministic cache fixtures, not the user's sample files. Root-validation
coverage refuses missing, short, offset-mismatched and changed processing roots.

The original/prospective preview API is also the seam for Pulse2781's separate
Shift-stretch feature. A changed root needs its own tempo-aware inverse mapping;
the retained-root crop path refuses it until that mapping is supplied. Stable
sampling during resize remains the separate Pulse2764 / PR372 work.

Native acceptance through the coordinator's serial Mac lane should use an
affected user sample and an untouched control on the approved optimized source:
continuously trim both edges with Warp on/off, include a processed Warp-off clip,
loop/partial-source case and zoomed/offscreen edges; confirm waveform peaks stay
visible during motion and after release, newly exposed/cropped content is correct,
and canonical timing, fade behavior and one Undo/Redo remain correct. Capture the
source/build label, sample identity/rate, modifiers and a bounded drag recording.
No app replacement, native job or save/quit is performed by this cloud scope.
