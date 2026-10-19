# Pulse2861 / PR463: stop resize panning on interior pointer return

Frozen source c0cbec36990546ceefbba22656d3819ddd143e5a / tree07236aae104738faa7a41d1cc257f2828406ce52, canonical branch pulse/task-2861-g1-4ead7d594586, base9c2f76ad19253c7a17c78053c1a5883d827c1ed9 (ordinary main7fe3c887). Two commits retain test-only6b6c6efc actual RED then minimal production relocation. Five files only; no foreign source imports.

Actual audio and MIDI rendered capture/ticks move the original canonical clip handle out of the viewport. Child-local drag callbacks disappear while the active preview/timer remains. Original two cases fail: interior pointer1113.5 leaves payload atedge1740; viewport192000 advances195600. The fix relocates those exact typed/current/stale/modifier/preview handlers to the stable existing editor root; no new state, timer, capture owner, propagation suppression, viewport/model/worker law or track-height behavior. Remove child callbacks to avoid duplicate delivery.

Four added tests cover both edges for normal audio, MIDI, Shift stretch, and permanent Warp: actual original handle unmount; interior stops while capture remains; reentry resumes; crossing opposite edge reverses; second interior stops; real worker geometry exactly matches last preview; one full Snapshot Undo/Redo. Original enabled-Warp outward setup was unsupported retained-root geometry and never unmounted the handle (active=true, preview=None); preserve its raw3PASS1FAIL plus isolated diagnostic. Correct fixture uses valid inward Warp crop, not weakened assertions or new admission law.

Final clean12-stage component qualification: 21pan +18release +9terminal FIFO +6trim pure =54 distinct runtime cases, zero ignored. Both default/feature UI strict Clippy, architecture, UI style, GPUI/dependency provenance, fmt and diff checks. Historical anticipated11 terminal count corrected to actual9; no zero-match. The controls-v2 4-stage result is earlier source-dirty evidence, not frozen receipt. No full21 claim. No native observations/performance/parity claims.

Protected proof retains old tests/docs/todo as byte prefixes and unchanged autoscroll/controller/release/terminal/model/Session/Engine bytes. Runner imports exact accepted688ce8d3 guarded local-ci, Cargo steps use normal target ownership, no manual shared-target clean. Scripts/raw failures/positive logs/source blobs are sealed.

Root-only finite optimized native acceptance remains open: audio/MIDI both resize edges, pan until original handle leaves screen, held move interior stops, reenter/cross reverses, Shift and enabled-Warp valid inward crop, release exact preview and Undo/Redo; Escape/stale project existing behavior. No source-agent app operation. This does not claim Pulse2867 outside-source-track release is fixed; it is next independent canonical task. Sole merger owns current-main composition, required full21, fresh review and merge; Pulse remains In Progress until actual merge.

Exact clean receipt:
{
  "source": "c0cbec36990546ceefbba22656d3819ddd143e5a",
  "seconds": 254.83,
  "summary_sha256": "6571a816c886fa1ee7b2c140eadaa5d356fe6f4b38136d490b89997e62010dba",
  "content_sha256": "6158b5a7819b62362a2c9b23268f9bd4e3d9edd7e1bf10a4a8f3cecb098269d3"
}
