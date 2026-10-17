# Clip conversion

Pulse #2658 / [PR274](https://github.com/PORTALSURFER/hex/pull/274), requested
17 October 2026. Clip View exposes `→ audio` for MIDI and `→ midi` for audio in
Arrangement and Session. Controls are unavailable while edits are pending; the
worker checks the captured project generation/revision before conversion.

MIDI to audio renders this clip's instrument to a project-rate stereo float32 WAV
on the control worker, then replaces its source in place. Track, ClipId and
Arrangement frame endpoints stay fixed. Effects, track mix, automation, sends and
Main processing remain live. Other clips and input routes are excluded from the
bounce. Instrument host output gain remains live; the stored source compensates
for the retained instrument input gain. Float samples above full scale survive
without clipping or PCM16 quantization. Finite source loops and tempo changes use
the shared prepared renderer. Mutable instruments use the existing bounded
preroll path at nonzero Arrangement starts.

Arrangement bounces end at the clip boundary, cutting instrument release beyond
that boundary. Session audio remains a one-shot: bounce the initial pass from the
source start through loop end (source end for a non-looping source), using the
current tempo map from beat zero. Names, cell keys and ClipIds survive conversion;
Session looping is not preserved as an audio loop.

Audio to MIDI discards clip audio/fades/pitch/warp/gain/source markers and creates
an empty one-shot MIDI source. Arrangement start and end frames are converted
independently through the canonical tempo map. Session source duration is converted
from frames at beat zero. The result opens in the normal editable piano roll.
There is no note detection, transcription or restoration of previously bounced
notes. Undo restores the complete original source in one step; redo restores the
conversion. Existing assets remain retained for history and save compatibility.
No persistence schema change is needed: replacements use existing clip kinds.

## Media ownership and refusal

Generated files live in unique create-new folders under
`~/Library/Application Support/hex/media/bounces/`; canonical assets store absolute
paths, so save/reopen does not depend on resident memory or a temporary directory.
WAV staging/publication reuses hex-export's synchronized create-new machinery.
Decode, waveforms and the replacement graph prepare before canonical history is
committed. Failed edits preserve the original clip, allocator, history and audible
graph. Failed post-render preparation removes its new unreferenced WAV; exceptional
publication recovery paths remain available. Successful media stays retained across
undo and later conversions; orphan-media collection/portable media bundling are
separate work.

Bounces admit at most 8,000,000 frames plus the existing resident asset/frame and
waveform limits. Preroll retains the engine's work ceiling. Verified streaming
bounces, mutable-instrument source loops unsupported by the linear export kernel,
and active effects before the instrument refuse visibly. Move those effects after
the instrument to bounce without changing the processing order. These are explicit
limits; conversion never silently removes devices or changes shared track routing.

## Validation and native acceptance

Focused source tests cover float PCM including above-full-scale values, non-neutral
instrument trims/effect/mixer levels, tempo-crossing placement, analytic source
loops, feedback instrument preroll, isolation from other MIDI, Arrangement/Session
identity/history, saved media reopening, empty-source editing, failed output paths,
pre-instrument effect refusal, stale worker requests and conversion hit targets.
Full local-CI results belong to the exact PR candidate and retained run summary.
Linux/source results do not establish native macOS interaction, listening or timing.

User checklist for an optimized macOS build:

1. Select MIDI in Arrangement, choose `→ audio`, listen and check position/length,
   effects and mixer levels. Undo/redo, save/reopen and listen again.
2. Select audio, choose `→ midi`, draw/edit notes in the empty piano roll and play.
   Undo should restore the original audio and its source settings.
3. Repeat in Session; check the same cell/name and one-shot bounced source.
4. Try a long clip or active effect before the instrument; verify a useful refusal
   and unchanged original clip. Native acceptance remains pending user results.
