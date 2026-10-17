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
