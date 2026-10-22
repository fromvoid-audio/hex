# Live knob reference observations

Installed Live rechecked 11 October: 12.4.6 (2026-09-10_0de5c8fa9a),
arm64; executable SHA256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
Existing stopped Live window was inspected read-only; one stationary click
selected Osc A Coarse Frequency, preserving its value1. No reference project,
transport, audio setup or parameter value was edited or saved.

Ignored private captures under `research/private/knob-focus-20261011/`:

- `live-tone-focused.png`: SHA256
  `0fa007cdfaf99498f30bab0a578761ecdf0a309f92ee3d2989430aa3ab5d6674`.
- `live-coarse-focused.png`: SHA256
  `42d3067cf5e7da6ffed9cf3a3493943add9be9e22ad93030f3d3e4947f550574`.

Both show four small gray L-shaped corners, including the label and readout.
They do not show a continuous or chamfered rectangle. At2x capture scale each
arm is approximately6physical pixels (3logical), with about2physical pixels
(1logical) thickness. Coarse spans roughly116x80physical pixels (58x40logical),
while its dial diameter is about36physical pixels (18logical). These are
manual raster estimates, not authoritative accessibility hitbox measurements.

Tone100% has cyan filling the full rail. Its arc begins at six o'clock and
ends at three o'clock, leaving the lower-right quadrant for the readout.
Time0% is a bipolar center value: its needle points upper-left; numeric zero
must not be treated as normalized minimum. Coarse1 is near its lower endpoint,
not proof of the exact minimum. The user separately observed Live bottom-gap mixer/Return knobs and pan with a
center-up needle and a small arrow. The early captures do not establish exact endpoints, hitboxes or transfer laws.
Reference images are private evidence and are not shipped or committed.

## disposable Live endpoint captures

Following direct human current-idle authorization, the disposable Untitled set
was inspected with transport stopped and audio off. Track3 pan was moved to
AX value-1/details50L and value1/details50R. Clicking the observed triangle
reset value0/detailsC. Its bottom-centered opening and neutral reset behavior
support the Pan semantic class. SendA reached minimum value0.0003162278/details
-inf decibels and maximum1/details0.0 decibels; an intermediate value0.462381
reported-6.7 decibels. That intermediate is not a calibrated normalized midpoint.
Send and pan were restored to minimum and center, respectively.

Operator D Coarse reached AX value0/details0.5 and value48. An initial upward
drag reached33; the maximum capture was replaced only after the true48 endpoint
was verified. These captures support the lower-right opening and black minimum
rail/full cyan maximum rail. A fresh user-input guard blocked the middle-value
attempt before delivery, so automation stopped and its native lease was released.
No project save, playback or audio setup change was performed; Coarse D remained
48 in the disposable unsaved set. Later passive observation found the human had
disabled D; no automation attempted to change that state.

Private images and SHA256 inventory are in
`research/private/knob-focus-20261011/endpoint-captures.json`. Pan left/right/reset
captures are live-pan-left.png, live-pan-right.png and live-pan-neutral-reset.png;
send range captures are live-send-minimum.png, live-send-intermediate.png and
live-send-maximum.png. Coarse captures are live-coarse-minimum.png and
live-coarse-maximum.png. All are3600x2130 at the same2x scale; cropped inspection
files preserve scale. The cursor overlaps some needles, preventing precise
needle-pixel measurements. No exact marker/hitbox/transfer-law parity is claimed.
