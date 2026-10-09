# MIXER-SOLO-001 — exclusive and additive solo

Status: documentation reviewed; native reference experiment planned.

- Reference identity rechecked 8 October 2026: installed Live 12.4.6
  (2026-09-10_0de5c8fa9a), universal arm64/x86_64 executable,
  SHA-256 5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949.
- Primary documentation: [Live 12 mixing manual](https://www.ableton.com/en/live-manual/12/mixing/),
  sections 18.1 and 18.6, retrieved 8 October 2026.
- Documentation observation: ordinary track solo suppresses other track outputs;
  default selection is exclusive, with Cmd on macOS enabling multiple soloed
  tracks. Routed tracks and returns have additional rules.
- Fixture/render/settings/hardware measurements: none captured; no binary
  function decompiled for this question. No output hashes or numerical parity.
- Independent Hex scope: straight Arrangement tracks without routing/returns.
  Toggle resolved atomically against canonical state; exclusive enabling clears
  other solos, additive enabling preserves them, disabling affects that track.
- Explicit Hex edge policy: mute still suppresses a soloed track. This particular
  precedence is not established by the documentation or a native experiment.
- Deferred: groups, sends/returns, routing-aware solo propagation, cue output,
  multi-selection, persisted exclusive-solo preferences, exact Live mute interplay.
- Planned distinguishing experiment: two original user-authored impulse tracks;
  ordinary solo, Cmd-additive solo, mute the soloed track, toggle solo off,
  export without warp/fades/effects/normalization/dither. Record output settings,
  alignment, hashes and observations before claiming matched behavior.
