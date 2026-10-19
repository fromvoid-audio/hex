# Pulse2867 / canonical PR469: captured resize release outside source track

Frozen source5733384ccb2bc26df8fdd09b642312fdbc85ea96/tree19445fc09d660d4b736acac7afaec91433519d63 on canonical pulse/task-2867-g1-1d27570a68d6. Original canonical6d38e728c938972a9c41219b554a0086001d35e6 is retained. One ordinary merge of published463/c0cbec36 created b9e0e3afdcf05dd8c6c4a9331d1b822606151c0d, exact same tree as463. Dependency is explicit; own delta is five files +248/-46, not the entire stacked PR diff. No moving-main chase.

Actual cause: finish_trim_pointer_release formerly required a hovered Arrangement lane or explicit browser exception. Other-track releases passed that foreign track to drop_trim, which rejects it; non-lane releases fell through to cancel. Captured resize ownership must stay with its original clip/track. The entire production delta is six inserted/seventeen removed lines within this existing helper: remove receiver eligibility gate, use drag.source_track. Keep root routing, horizontal retained-point/final-coordinate/modifier resolver, model/source/Warp admission, focus/current/stale guards, pending FIFO and real canonical worker terminal. No new state/owner/timer/queue/API or model/DSP change. Legacy browser boolean argument remains as an unused internal caller seam to avoid unrelated wiring edits.

Negative evidence is deliberately separated:
- red-v2: actual other-audio and other-MIDI captured-release canonical mismatches prove two product failures. Third failure in that same run was a no-op fixture/history assertion, NOT an independent product RED.
- first green: those two product regressions pass; same no-op assertion remains. No second production attempt was made.
- outside-corrected-red: corrected generic-outside x retains changed left preview108200..192000; exact pre-fix production leaves canonical96000..192000. Actual0PASS1FAIL0ignored0.68s is authoritative outside baseline.
- first red attempt was a fixture compiler error from private MidiClipTrim fields; corrected to existing accessor methods before actual execution. All logs/source checkpoints remain.

Test-only fdee3bcb and2b2299f3 preserve these stages before final production commit. Same minimal production patch is reapplied byte-exact after baseline terminal. Preliminary controls pass seven receiver tests plus three pan chains. Final source adds explicit unchanged selected-track/clip, errorNone, exact retained audio command-frame assertions. Both-edge real pointer tests cover normal audio/MIDI, Shift stretch and enabled Warp over other tracks, generic outside background, actual header, ruler, status bar and browser. They compare complete committed clip/source metadata, preserve the other track, clear capture/pending, and perform one full-snapshot Undo/Redo. Pan chains use real timer progression until the original handle unmounts, interior stop/re-entry/opposite reversal, then actual ruler release and worker/history. Former cancellation oracles are intentionally revised for the user's new requirement, not silently removed.

Final clean12-stage component checks include all22 release,24 pan,9 terminalFIFO and6 trim controls =61 distinct runtime tests, zero ignored; both default/feature strict UI Clippy, architecture/UI style/GPUI/dependency provenance/fmt/diff. See exact raw receipt below; preliminary dirty receipts are not qualification of this frozen source. Existing actual asynchronous worker refusal, queue/FIFO/earlierACK, source bounds, snap/finalMouseUp reversal/modifiers, Escape, focus loss, mode change, stale generation and source-replacement controls remain in these owning groups. No full21 claim.

Protected bytes include unchanged render.rs (preserves463 root pointer delivery), autoscroll/controller/terminal/model/Session/Engine/runner/Cargo. Sealed before/after blobs, exact production patch, original failures and positive logs are source-bound. Uses existing accepted688 guarded local-ci component runner with Cargo target ownership; no shared-target cleaning or native operations.

Root-only optimized native checklist remains open: audio/MIDI left/right ordinary and Shift/permanent Warp, drag valid preview then release over another track/header/ruler/blank/browser/detail panel; same source exact extent and oneUndoRedo, other track/selection untouched. Pan until original handle leaves screen, move interior to stop then release outside source lane; final valid/unsupported pointer and stationary modifiers; Escape/lost focus/stale project must cancel, actual worker refusal must retire truthfully. OS outside-window capture is qualified only where the actual platform delivers captured release; cloud GPUI delivery is not native capture/GPU/waveform/audio/listening acceptance.

Sole merger owns exact current-main composition, required full21, fresh review and merge. Pulse remains In Progress until authenticated merge; source-ready does not mean installed/native accepted. Preserve463 and all prior source/evidence packets unchanged. Next general queue remains critical resize neighbors under Root coordination, no extra worker.

Exact clean receipt:
{
  "source": "5733384ccb2bc26df8fdd09b642312fdbc85ea96",
  "seconds": 287.376,
  "summary_sha256": "83d7c3cc9250ba2adaebaf27b51b7bb7767a48ef79ecc76444f1bcca82f00ae8",
  "content_sha256": "a5f5e731ab2430056fb9eaa91b2ab0aea5e2b50bfff6637273e0cf5adb2a176b"
}
