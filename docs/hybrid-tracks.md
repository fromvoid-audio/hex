# Hybrid tracks

Implemented and source-accepted, 9 October 2026; user native acceptance pending. This is a user-selected Hex
deviation from Live, not a reference compatibility claim.

Every canonical track owns an audio clip collection and a MIDI clip collection
under the same stable TrackId and the same name/gain/balance/mute/solo controls.
Clip kinds describe source data, not a track capability. Audio remains frame
anchored; MIDI remains beat anchored. Existing overlap behavior is retained: when
both Arrangement sources overlap, their contributions sum through the shared
track mix. The original Hex sine instrument renders MIDI in this step.

Session retains one sparse audio, MIDI or explicit-stop cell per (track, scene).
Different scenes on the same track may contain different source kinds. Launch
ownership still selects one cell; adding hybrid capability does not layer two
Session cells on the same track or change quantization/Stop/return policies.

## Ownership and persistence

hex-project owns both collections, stable IDs, validation, edits and history.
hex-engine compiles both into one prepared track and retains current work/voice
budgets. hex-session performs candidate preparation before committing edits and
coordinates media/save/open on its worker. GPUI actions choose their editing
controls from the selected clip or cell. No second project or mixer model.

Schema 13 encodes both clip collections per track. Strict legacy schemas
1–12 retain their existing readers: audio contents migrate to the audio collection,
MIDI contents to the MIDI collection, with the other collection empty. Track
order/IDs, mixer settings, sources, notes, fades, phase, tempo/meter and Session
references must survive migration. No track splitting, ID regeneration or silent
content loss. Unknown fields/malformed references retain explicit refusal.

Combined collection limits and global ID high-water validation must cover both
clip kinds. Audio/MIDI edit destinations require an existing track, not a type.
Session moves still require an empty destination cell. Failed edits/preparation
retain allocator/history/dirty state and the last valid audible graph.

## Acceptance

- Model commands and undo/redo on both clip kinds sharing one track; atomic
  malformed/over-limit refusal and globally unique identities.
- Schema-13 mixed-track round trip and representative literal legacy migrations.
- Independent audio-plus-MIDI Arrangement PCM sum with shared mix, correct
  endpoint/export length and no duplicate prepared track identities.
- Audio/MIDI Session switching on one track, including release tails, Stop,
  replacement/handoff, copy/move and occupied-cell refusal.
- Work admission covers simultaneous Arrangement MIDI and outgoing Session
  releases under the current per-track/project limits.
- Candidate worker preparation, save/reopen and UI action coverage; no callback
  allocation/locking or final reclamation introduced.
- User native test checklist after the build is available. Native UI and listening
  acceptance are recorded separately from source/headless results.

Deferred: instrument/device chains, plugin hosting, recording, new routing,
clip overlap-policy changes, browser preview/drag and full Live-set fidelity.

## Independent core checkpoint

Root's separate public-API acceptance binary passes on the current implementation:
one AddTrack owns overlapping audio and MIDI in Arrangement, compiles to one
prepared track and renders the exact source sum over 40,000 frames with irregular
block partitions. Independent audio-only/MIDI-only projections retain shared
gain/balance and prove MIDI is audible. Schema-13 encode/decode retains the exact
snapshot, including audio and MIDI Session cells on that same track. Mixed MIDI
move/undo/redo preserves canonical history.

Evidence: `artifacts/hybrid-root-acceptance.log` and
`artifacts/hybrid-acceptance/src/main.rs`. The original float WAV and user-openable
metadata fixture are `artifacts/hybrid-acceptance-source.wav` and
`artifacts/hybrid-acceptance-project.hex`. Both debug and release acceptance runs pass. No native playback/listening claim
follows from this test.


## Integrated acceptance and review build

Workspace tests, strict workspace Clippy, desktop build, formatting and both
architecture/style guards pass. The GPUI support suite passes 190 tests with one
existing ignored test. Literal schema-11 Session and schema-12 audio fixtures
cover migration alongside older fixtures. Worker import/save/reopen and same-track
Session audio→MIDI→audio switching, including outgoing MIDI releases, pass.

The headless production callback probe compares simultaneous same-track audio and
MIDI against the exact sum of audible isolated sources with shared gain/balance.
All six callback allocation tests pass in debug and release, requiring zero total
and signed-net allocation counts/bytes; setup, oracles and reclamation occur
outside measured render scopes. This does not verify native device deadlines.
Evidence: `artifacts/hybrid-workspace-tests.log`, `hybrid-gpui-tests.log`,
`hybrid-callback-allocation-debug.log` and `hybrid-callback-allocation-release.log`.

Separate review build: `artifacts/hex-hybrid-tracks-20261009.app`. Existing running
apps and unsaved projects are preserved. The browser right-edge resize fix is
included. User native checklist:

1. On an existing audio track, use cmd+shift+m and add MIDI notes; play both
   sources, then save/reopen and confirm both remain on that track.
2. Select a track containing MIDI and double-click a browser WAV; confirm the
   audio is imported onto the same track and both remain editable.
3. Put audio and MIDI in different Session cells on one track; launch each,
   switch between them and Stop, checking sound and release tails.
4. Drag the browser right edge in both directions, release, and collapse/reopen.

Native interaction/listening results remain pending user feedback. The broader
roadmap remains paused.
