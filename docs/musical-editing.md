# musical editing — pulse 27 / pr 185

The canonical issue requires clip gain and pitch, all six requested Live-style
warp modes, beat-anchored audio, source envelopes, metronome, scrub and loop
handles, snap nudges and drag autoscroll. The user clarified on 16 October 2026
that Beats and Re-Pitch come first, followed by Tones, Texture, Complex and
Complex Pro. Distinct algorithms and meaningful output checks are required;
mode labels alone do not establish either functionality or Live equivalence.

## implemented source stages

Clip gain uses the canonical model/history/storage and common resident/streamed
rendering described in [clip view](clip-view.md#canonical-audio-clip-gain-musical-editing-issue-27--pr-185).
Snap nudges retain [their directional landmark contract](snap-nudges.md).
Existing Arrangement transport-loop handles use the typed loop rail in
[`loop_rail.rs`](../crates/hex-ui/src/editor/loop_rail.rs), with
[rendered gesture tests](../crates/hex-ui/src/editor/loop_rail_ui_test.rs).
These stages do not complete the full issue.

The transient metronome belongs to interactive `Playback`, outside saved project
history and linear export. Its normal and accented 20 ms click samples are
prepared on control, with at most 30,720 sample bytes at 192 kHz. The owning
playback graph retires on control. Callback rendering retains scalar sample
position and expected Arrangement frame, without allocation or ownership copies.
Prepared canonical tempo and meter provide denominator-beat landmarks; meter
markers begin accented bars. Arrangement transport remains the clock when
Session clips own track routes. Seek, Stop, pause and loop wrap discard the old
click tail; a pulse exactly at the new position retriggers. Compatible revision
replacement preserves the toggle and tail; changed timing reanchors. Legacy
and linear-export graphs refuse enabling the click. The toggle is admitted
through the existing bounded command queue. The UI displays acknowledged state
and prevents another toggle until the current request is consumed or rejected.
The click is a monitoring signal after the musical mix, independent of track
and master gain. It is not a separate hardware cue output or a physical timing
acceptance claim.

Captured Arrangement clip move/copy, browser-audio and audio/MIDI edge trims now
scroll horizontally while a held pointer is within a timeline lane's 24 px edge
bands (capped to a quarter of narrow lanes). Scrolling uses the existing viewport
horizon, at 1.5 viewport lengths per second, with at most 50 ms credited to a
single delayed UI tick. It does not extend the horizon or scroll vertically.
Each payload shares its internally advanced viewport with the native drag;
source grab offsets and musical anchors remain captured. Preview recalculation
does not edit project/history. Release still uses the existing canonical edit;
manual viewport changes, source revision changes, busy edits and canceled drags
cannot apply a stale preview. Entering the browser or toolbar does not scroll.
Time-selection/range scrolling retains its existing independent contract.

Ruler scrubbing is drag seeking through the existing transport, preserving the
current playing/paused state. It is not stopped-transport audio audition. Pointer
events replace one latest target; the UI tick admits at most one seek after
pending command/publication and seek acknowledgment work clears. A released final
target survives queue pressure. Changed project identity, viewport/geometry,
focus, edits, Escape or Stop cancel future targets, without claiming that an
already admitted seek was undone. Prepared stateful seeks retain their existing
bounded worker and publication-fence lifecycle.

## canonical musical audio

Arrangement audio owns an optional immutable processing root and a crop view.
The root retains its original source interval, loop phase, settings, markers,
source gain and fade context. Splits, range fragmentation, copies, moves and
handle trims retain that root. Preparation renders the full root once, then
crops its output: splitting does not restart a resampler, grain or FFT, and does
not round source positions independently. Shared roots and prepared PCM survive
history/snapshot/save. Explicit numeric source/length edits establish a new root;
source bounds cannot be changed while retained warp markers exist. Handle trims
can shrink/reextend within the retained root and refuse extension beyond it.
Pitch edits on cropped clips remain rooted at the original processing origin;
they deliberately change the source position reached at the crop start. Reset
the source region explicitly when a new pitch origin is required.

A frame placement migrates unchanged. Musical placement stores Q32.32 quarter
beats for its start and exact local view span. Tempo edits resolve each endpoint
independently, then subtract frames; the canonical command and all preparation
must succeed before metadata/history changes. Unwarped musical clips reproject
their outer span while keeping source rate, one-shot exhaustion silence and
loop policy. Warp requires musical placement and enabling it captures the
current exact endpoints. Marker source frames and local beats increase strictly,
including explicit region endpoints. Moving a cropped tail to beat zero derives
a signed *internal* project reference from placement minus crop start. Only its
invisible finite analysis prefix extends tempo-at-zero backward; authoring,
source and crop coordinates remain nonnegative. Signed nearest-frame half ties
move later, including negative halves.

Pitch is integer cents in -4800..4800. Unwarped playback reads the original
source at `2^(cents/1200)` without changing placement/duration. One-shots become
silent after exhaustion; slower reads are cropped and loops repeat only their
declared source region. Fractional position is derived from the root origin,
without accumulating per-sample rounding. Re-Pitch stores transpose but leaves
its independent pitch control inactive: timing sets its playback rate.

Source gain is a sorted original-source-frame envelope, at most 1024 points.
Q16.16 linear multipliers cover exact zero and unity through 4×; interpolation
is linear with endpoint hold. It is applied before transformation and follows
source content/loops. Output fades remain separate. Existing frame fade laws
are preserved exactly; musical roots freeze their fade projection so splits and
tempo changes preserve the same musical fade. A faded musical clip currently
refuses switching back to frame anchoring; clear its fade first. This avoids
silently changing a cropped frozen fade. There is no source-pitch envelope.

Session audio remains an unwarped one-shot with clip gain. Arrangement-to-Session
transfer refuses retained musical/transform metadata rather than discard it;
ordinary gain transfers remain supported. Streamed clips retain the unchanged
neutral direct render path, including phase/fades/gain. Nonneutral pitch, warp,
source envelopes or musical frozen fades require resident audio; preparation
refuses those streamed changes before canonical commit. Resident import limits
are unchanged.

## six independent processing modes

| mode | implemented algorithm and meaningful controls |
| --- | --- |
| re-pitch | 64-tap, 256-phase Blackman-windowed sinc reconstruction with rate-dependent anti-alias cutoff; timing changes pitch |
| beats | shared stereo-energy transient analysis or divisions inverted through the actual nonlinear marker map; pitched attacks, off/forward/alternating tails and decay |
| tones | local normalized-autocorrelation period estimation, pitch-synchronous overlapping windows and waveform-similarity fallback; grain size changes analysis/window extent |
| texture | fixed windowed grains, waveform alignment and deterministic stored seed/fluctuation, independent of period estimation |
| complex | original radix-2 FFT, linked stereo phase propagation, shared peak/identity locking and transient resets |
| complex pro | complex spectral core plus smooth log-spectral-envelope reimposition during transpose; real formant amount and envelope resolution, exact no-op correction at zero transpose |

Phase-vocoder design follows the published [Laroche/Dolson analysis](https://doi.org/10.1109/89.759041)
and [stereo phase treatment](https://arrow.tudublin.ie/argcon/66/).
Spectral-envelope correction is informed by [Lenarczyk 2017](https://www.isca-archive.org/interspeech_2017/lenarczyk17_interspeech.html).
Implementations are original. These signal contracts and finite synthetic
fixtures do not establish Ableton's private algorithm identity, general listening
quality or exact Live equivalence. Spectral phasiness, grain artifacts and
transient detection errors remain possible on untested material.

The lowercase Arrangement controls dispatch canonical commands for pitch,
frame/beat anchoring, warp enable, all modes and their actual parameters, source
marker positions/local beats and source gain points. Point controls are paged
eight at a time, clamped to the selected clip. Source/length controls retain
the explicit new-root/refusal behavior above. No inactive decorative modes are
shown. The worker supplies accepted root PCM plus crop coordinates for every
transformed clip, independent of transform cache eviction. Session builds and
deduplicates immutable peak summaries off UI/callback; at most 262144 leaf bins
per revision, at most 1024 per root, with coarser summaries in dense projects.
Output peaks already include the root fade/source envelope; display applies clip
gain once. Existing fade controls read retained authored reference-frame durations
with an explicit retained/reanchor disclosure; reset/adjust inspect that retained
fade. A bounded read-only projection supplies the actual frozen-tempo fade curve
and its knots, shared with preparation; the overlay does not multiply peaks.
The source editor deliberately shows original source coordinates and markers
separately. Unprepared transformed drag/trim previews show geometry
without a misleading original 1:1 waveform; accepted output appears after prep.

## preparation and retained memory

`hex-warp` depends only on std and owns pure bounded PCM preparation. The engine
adapter translates project metadata, selects direct/prepared playback for every
compiler entry point (native, offline and CLI), and owns a control-side cache.
The Session preparation transaction includes graph/inventory validation and
waveform construction. Errors and cancellation keep the prior project, history,
accepted sound and peaks. Existing service pending/error events report work;
dropping the service requests bounded shutdown cancellation without joining or
canceling normally queued edits merely because newer edits arrived.

Bounds: 1024 markers/envelope points; 8192 projected segments/attack events;
local stretch including pitch compensation 1/8..8; 16777216 full root output
frames, including invisible analysis prefix; 32 MiB job scratch; one billion
conservative work units per root. Long or dense roots can be refused within
these finite limits even when a tiny crop is visible. Multi-root transactions
may remain pending longer; cancellation is polled throughout each preparation.
There is no cache-dependent transaction work ceiling that would prevent a
previously accepted project from cold-loading. Mapping/analysis/normalization
are bounded; normalization uses a ring rather than a full-output weights array.

Derived stereo f32 PCM has a process-wide 768 MiB retained-payload ceiling.
Reservation precedes output allocation. The underlying Audio allocation owns
its RAII token, shared by cache/candidate/active/retired graph and waveform
references; cache eviction does not claim audible memory was freed. Originals
retain their existing resident budget. The derived cache is additionally limited
to 256 MiB and 256 entries. Full original-source owners retained by committed
and pending cache entries share a separate 256 MiB ceiling, conservatively
charged in full per entry even when sources are shared. Oversized entries bypass
caching without changing import limits. Pending and committed derived entries
share the same 256 MiB ceiling; pending entries also obey the 256-entry limit.
Every audible/candidate buffer remains charged to the process-wide retained
ceiling. Entries retain the real source owner and complete root,
conversion, tempo projection, parameters, seed and algorithm version identity.
Clip gain is excluded, so changing only gain reuses prepared PCM. Buffers and
tokens retire through existing control reclamation. Render performs no transform,
reservation, cache work, allocation or final ownership drop.

## reference and algorithm boundary

The required mode families and control behavior are described in the public
[Ableton Live 12 warping manual](https://www.ableton.com/en/live-manual/12/audio-clips-tempo-and-warping/#warp-modes).
In particular, Re-Pitch derives pitch from playback speed; its independent
transposition controls are inactive. An independent implementation must state
its own bounded algorithms and signal checks. No proprietary DSP code or assets
are copied, and a mode name does not establish matching output.

The installed reference was rechecked on 16 October 2026: version
`12.4.6 (2026-09-10_0de5c8fa9a)`, executable SHA-256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`,
matching [the recorded reference](research/reference.md). This is identity
verification, not a new native behavior or listening experiment.

## optimized preparation measurement

On 16 October 2026, `cargo run --release -p hex-warp --example profile --locked`
measured one second of original 48 kHz stereo synthetic partials prepared to
72000 frames (+300 cents where independent pitch is active). Apple M5 Pro,
arm64; `Instant`, one warmup and five measured repetitions per mode, including
output allocation but excluding output destruction/checksum. Machine load was
not controlled. Median preparation milliseconds were Re-Pitch 11.836, Beats
28.868, Tones 80.950, Texture 44.954, Complex 136.319 and Complex Pro 132.463.
Peak reported scratch was 811152 bytes. The raw report includes min/max times,
work units and deterministic PCM checksums:
`artifacts/local-ci/20261016T061058Z-184eb5b1/08-warp-prep-profile.log`.
The retained `artifacts/musical-editing/perf/warp/measurement.json` records exact
kernel hashes and fixture/method. The enclosing development run was marked
stale as other source changed; this isolated measurement is not final CI,
throughput under competing jobs, a physical callback deadline or listening
acceptance. Final neutral compile/render comparison belongs to the integrated
candidate and the separately retained 37180a5 baseline.

## verification and optional native checklist

Source tests and native evidence remain separate. Run the full local runner on
the clean final commit before readiness/merge; targeted development runs and
source guards do not replace it. Include the callback allocation probes for
metronome and clip gain. New tests cover click timing/partitioning/seek/loop/
inheritance/export refusal, rendered clip scrolling and stale cancellation,
bounded delayed ticks, and the gain cases listed in clip view.

The optimized exact-source candidate should be checked with this finite list:

- Adjust Arrangement and Session clip gain; hear independent levels, inspect
  waveform height and verify Undo/Redo and save/reopen.
- Enable the metronome while stopped and playing; check bar accents and tempo/
  meter changes, seek, pause, Stop and transport-loop wrap. Export stays click-free.
- Hold a clip move/copy or audio/MIDI trim near each horizontal edge; preview
  follows scrolling and release makes one edit. Cancel and Undo preserve source.
- Drag browser audio near an edge; the final import uses the scrolled destination.
- Check ruler drag seeking and final release after queued or prepared seeks.
- Adjust pitch and source gain, split a loop with a nonzero phase/fade, move its
  tail to beat zero, Undo/Redo and save/reopen; check accepted output peaks.
- Enable beat anchoring, edit tempo across the clip, and compare all six modes.
  Check Beats attack/tail/decay, Tones grain size, Texture grain/jitter/seed,
  Complex stereo content and Pro formant/envelope settings on transposed speech.
- Add/move/remove source gain and warp points; page to another clip and verify
  controls remain reachable. Confirm invalid bounds show a useful error.

Physical audio deadlines, listening quality, native interaction, Windows and
reference compatibility are not established by these source tests.
