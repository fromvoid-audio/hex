# selected clip beats stretching

Pulse2771 / PR379 prioritizes a usable Warp enable and algorithm picker in the
existing selected Arrangement audio Clip View. These controls now appear below
the sample title, above position/length/source fields. Enable `warp · on`; the
initial algorithm is `beats`, with `preserve · transients`, forward tails and
100% decay (no transient-envelope attenuation). Direct choices retain all six
already implemented algorithms. Selecting the current algorithm preserves its
parameters. Disable/re-enable Warp to retain its saved map and settings.

Warp is sample stretching, independent of snapping and source-file edits. The
initial map preserves the sample's current duration; project tempo or existing
warp-marker beat edits change its duration. With Beats, transient starts follow
that map while each intervening segment plays at the chosen pitch. Forward
or alternating tails fill expansion; Off leaves silence after segment exhaustion.
Pitch remains independent of stretch duration (except the deliberately coupled
Re-Pitch mode). Existing advanced preserve/tail/decay, pitch, source envelope,
anchor and marker controls remain in the same scrolling sidebar.

This reuses the existing canonical `SetAudioClipWarp`, immutable domain/maps,
control-worker `hex-warp` preparation and prepared playback/export roots. No
second renderer, proprietary implementation, source rewrite, callback analysis,
new schema, raised resource ceiling or backward alpha compatibility is introduced.
Unsupported streamed warps and Session-loop warping remain explicit limitations
of the existing workflow; this selected Arrangement clip improvement does not
silently enable them. Full sampler goal3 and its resource-context work remain open.

## behavior and checks

The public [Ableton Beats behavior reference](https://www.ableton.com/en/live-manual/12/audio-clips-tempo-and-warping/#beats-mode)
informs transient segmentation, independent timing/pitch and tail options. Hex
uses its own existing implementation; no sonic parity claim is made.

Independent PCM oracles exercise automatic attacks on either stereo channel at
known piecewise-mapped times, exact unity through source trims/phase/loops,
independent duration and400/800Hz pitch, stereo scaling/channel swapping,
silence, short samples and1/8..8 ratios. Existing tests retain distinct tails,
decay, cancellation and bounded scheduling. A first new edge fixture rounded17
frames down below the declared1/8 ratio; the corrected fixture rounds its valid
output extent up. It did not reveal a DSP defect. Full DSP suite passes17 tests.

Additional checks cover Beats save/reopen and one-step Warp enable/disable
Undo/Redo; automatic-transient preparation with root pitch, envelopes, loop phase,
tempo changes, split recombination, arbitrary seek and block sizes; actual shared
native/export preparation including gains and return routing; and headless actual
pointer clicks on the visible toggle and direct algorithm choices. Exact-source
full local validation remains required before merging.

## coordinator-owned native acceptance

After this PR's exact source is verified and included in the launcher's one
coordinated app delivery, test a disposable imported drum-loop project. No Mac
execution, installation, app quit or replacement is dispatched by this cloud chat.
Prerequisites: coordinator pins the merged source/PR and full local-CI receipt,
normal user save/quit if delivery requires it; no force quit or unsaved data loss.

1. Select the Arrangement audio clip and open Clip View. Verify Warp and all
   implemented choices are visible above source fields without scrolling.
2. Enable Warp, choose Beats and Transients. Change project tempo both ways;
   listen for retained attacks with unchanged pitch. Change pitch separately.
3. Compare Off, Forward, Alternating and decay on a percussive loop; listen for
   clicks, dropped/doubled attacks and stereo image movement. Inspect trim/loop
   seams, mid-clip seeks and repeated play/Stop.
4. Disable/re-enable; choose another algorithm then Beats; verify actual sound
   and retained settings. Undo/Redo each edit once, save/reopen a disposable
   project, and compare exported WAV with playback.

Expected evidence: pinned source/app provenance, fixture rates/tempo/ratio/pitch,
pass/fail per step and bounded audible/UI reproductions. User pointer/keyboard
and listening interaction is required; source tests do not establish acceptance.

The first full run on888365e1 was deliberately interrupted after workspace/export
passes when UI compilation exposed an unused test import that strict Clippy would
reject. It is retained as interrupted, never full PASS. Both new actual-input UI
cases passed directly on the compiled test binary before removing that import.
The corrected candidate requires a new full run.

Full20/20 checks passed on clean20c8493722ec5ab53d8fb91483c36011eda89976.
The final merge then incorporates already-verified original-rate retentionPR370;
only todo.md conflicted. Both work-log sections were preserved, with this task's
entry appended to avoid competing insertions. Final integrated-source checks
remain required by the repository's exact-candidate/conflict-resolution rule.

The integrated7d2538c4 candidate also passed20/20 checks. During that runPR374
added the browser-preview lifecycle and its own work-log footer. Clip DSP/model/UI
did not conflict; the footer insertion did. Both records remain intact, with the
Beats entry now between stable native-work sections. The resulting final candidate
requires exact checks under the same repository rule; prior PASS evidence remains.

## click repair and control discoverability (Pulse2830 / PR434)

The new user listening report supersedes any inference of sonic acceptance from
previous PCM tests. Original synthetic regressions reproduced a0.3001 adjacent
sample jump at a Forward tail reset and a0.5 Off-to-silence jump, away from the
intentional source attack. Existing Off/Forward/Alternating and0..100 decay state
and commands were present; the controls were below source fields in the scrolling
sidebar. They are now composed beside the visible Beats picker: Preserve,
stop/forward/back-and-forth choices and the current transient envelope value.
The existing state, worker command, disabled/stale-capture protection and Undo
route are reused. Native layout/input checks remain required.

Forward sustain overlaps only the middle/tail with a bounded2ms smooth equal-gain
blend; it never loops the attack and does not attenuate a constant DC tail.
Alternating sustain reflects sample centers without duplicating its endpoint.
Transient segment reconstruction clamps filter reads to that segment, preventing
the next attack from leaking into a fractional-pitch tail. Actual stretched cuts
and Off exhaustion use a bounded1ms safety release, separate from the authored
shortening envelope. Envelope100 still has no intentional exponential shortening;
exact unity playback remains unchanged. Work accounting covers both overlap
reads, scratch/output ceilings and cancellation remain unchanged. Algorithm
version2 invalidates older prepared-cache entries. Other modes retain their
existing algorithms and reconstruction behavior.

Original listening fixtures can be regenerated without reading user media:
`cargo run -p hex-warp --example beats_quality --locked -- NEW_DIRECTORY`.
It creates original stereo73Hz percussive/DC-offset PCM and three48kHz,2x-duration,
pitch0/envelope100 WAVs using create-new files. Before and after originals have
the same hash. On this fixture, maximum PCM16 adjacent step outside±64 frames of
mapped attacks changed from0.25697 to0.00528 for Forward, and0.25895 to0.00641
for Off. Back-and-forth was already amplitude-continuous on this fixture; its
maximum changes from0.00388 to0.00681 with the end safety release. These are
fixture-specific continuity measurements, not a listening verdict or a claim
that every metric improves. Raw artifacts and hashes are retained in the source
handoff; intentional attacks are excluded from the seam measure.

Finite coordinator listening checks: compare original/before/after at matched
level; listen to sustained/DC tails, bassy attacks and an imported drum loop;
try slower/faster/fractional tempos, Off/Forward/back-and-forth and envelope100
then lower; check stereo placement, no repeated attack, Stop/seek/loop and export.
In the selected clip verify visible controls at compact width, current selection,
Preserve divisions, parameter sound changes, one Undo/Redo, split/copy and current
save/reopen. Physical listening/UI, exact final local CI and optimized delivery
remain coordinator/merger work. Sampler admission and unrelated catalog features
do not gate this bounded Beats repair.

## Beats WSOLA quality follow-up — Pulse2847 / PR449

The subsequent listening report reopened musical quality despite the earlier
seam repair. The Beats-only replacement uses stereo-linked, transient-anchored
waveform-similarity overlap-add. Physical attack exclusions remain separate from
chosen Preserve groups. Selected boundaries follow the actual map; unselected
attacks inside a fixed group retain native-relative pitch timing. A compressed
next group can omit late material, but cannot accelerate or replay its attack.
All tail modes share that first pass and bounded boundary treatment; Stop still
produces exact silence after its available pass. Envelope100 does not gate each
grain. Other Warp modes and Engine ownership remain unchanged.

A broader independent test exposed severe added residual on27.5Hz at ordinary
2:1 compression with the initial12ms search radius. A40ms radius found coherent
phase without lengthening grains. Version11 therefore screens the entire wider
bounded range using16 stereo points, fully scores eight separated finalists plus
nominal/predicted positions, and refines multiple full-score winners before the
final fractional search. Only fully scored statistics drive reconstruction.
Both screening and full previous moments are computed once per grain. Every
search stage remains charged; the concurrently live search reservation increases
from4096 to8192 bytes inside the unchanged32MiB scratch ceiling. The work ceiling
remains one billion units. Version11 invalidates earlier prepared-cache entries;
selection/PCM identity with earlier versions is not claimed.

Focused source checks pass43 Beats cases, including480 complete-Match bitwise
comparisons of exhaustive versus exact-pruned scans,20 screened-versus-full
stereo quality comparisons, seven chosen-group laws, and66 positive sub-bass
rate/ratio/pitch combinations with six explicit out-of-range validation cases.
Actual Engine preparation/cache/Undo/seek/partition cases and linear export pass.
All54 default renders of the six authorised CC0 acoustic recordings complete;
the16.82-second timpani2x cases consume897519062 Forward and840646204 Alternating
work units. These are charged-work observations, not a broad throughput claim.
Original failures and intermediate renders remain separate evidence.

The original recordings have no asserted publisher BPM. A separately documented
120BPM arrangement constructed from the authorised bass/snare recordings provides
additional musical material; it is not a publisher loop or commercial reference.
Fixed-gain comparisons and additional whole-file level matching retain exact
settings and gains. Neither automated residual/RMS tests nor WAV generation
constitutes hearing, Live parity, or user acceptance. Optimized native listening,
representative mixed music and final integrated validation remain open.
