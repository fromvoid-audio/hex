# MIXER-BALANCE-001 — stereo balance and master level

Status: documentation reviewed; independent implementation in progress.

- Reference identity: installed Live 12.4.6 (2026-09-10_0de5c8fa9a),
  universal arm64/x86_64; previously inventoried SHA-256
  5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949.
  No new binary analysis performed for this unit.
- Primary source: [Live 12 mixing manual](https://www.ableton.com/en/live-manual/12/mixing/),
  sections 18.1 and 18.4, retrieved 8 October 2026.
- Documentation: default stereo pan changes the two channel levels together;
  split stereo pan controls channels independently. The Main track receives the
  mixed signals. The documentation does not give numeric pan coefficients.
- Explicit original Hex policy: balance in [-1,1], left coefficient
  `1-max(balance,0)`, right coefficient `1+min(balance,0)`. Center is unity;
  endpoints suppress the opposite channel, without crossfeed or favored-channel
  boost. This is not established as Live's pan law.
- Master linear gain [0,2], default 1, multiplies the summed stereo output.
  No new limiter, clipping or smoothing is introduced. Existing WAV encoding and
  device output policies remain their own boundaries.
- Native reference experiment, output hashes, tolerances, device settings,
  listening and decompilation: not performed.
- Planned distinguishing experiment: original asymmetric left/right impulses,
  center/endpoints/intermediate pan and Main volume changes, with warp/fades,
  effects, normalization and dither disabled; measure coefficients/alignment
  before claiming parity. Split stereo pan, automation and click-free changes
  remain later contracts.
