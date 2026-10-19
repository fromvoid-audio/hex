# shared spectrum painting

Pulse2832 / PR436 uses one private `hex-widgets::spectrum_painter` for the
measured input/output traces in ResponseGraph and every master/selected-track
layer in `layered_spectrum_plot`. The real EQ and project-spectrum owners retain
frequency/dB normalization, sample-rate/Nyquist limits, membership, capture,
freshness/decay, cache lifetime and explicit user intent. No FFT, worker, DSP,
MTS, model, schema or per-device ownership changes belong to this paint change.

Each owner supplies its actual plot rectangle and calibrated normalized points.
The shared bounded command stream interpolates every supplied point exactly.
Conservative monotone Hermite controls remain inside each segment's endpoint
envelope; extrema retain zero tangents and cannot overshoot. Duplicate/tiny X
columns use finite zero tangents. This is display interpolation, not smoothing
or alteration of measured values. There is no invented frequency-domain edge
extrapolation: fill closes vertically beneath the actual first/last sample and
along the display floor. Nonfinite/malformed points are refused; floor-only
silence has no fill, empty/retired owner data has no spectrum paths.

Both production components paint all fills before their grid, then all thin
traces above the grid and below response curves/nodes/labels. Each fill uses its
trace color and a float-alpha gradient fading to zero at the floor. A shared
0.24 total ink budget, capped at0.14 per layer, keeps overlapping fills subtle
even at the existing257-layer limit; float alpha avoids dropping small layers
through byte quantization. Input/output/master/track colors remain caller-owned.
The same clipped rectangle applies to fill and outline, preserving alignment,
resize and inherited masks. Default graph owners without measured layers do
not enter the painter; response curves and their input/hit contracts are intact.

No extra full-layer point copies are made for painting. A cloneable iterator
supplies at most257 borrowed layers, each at most256 points. The command stream
adds one cubic per sample interval and three floor-closure commands for fill;
GPUI retains its existing antialiased path tessellator. Bounds and transformed
edges are finite and nonpositive plots submit nothing. Renderer cost and
physical frame deadlines still require measurement on actual native hardware.

## source evidence and native limits

The first new actual-component regression ran the original independent painters:
three geometry/budget checks passed and the production call-path check failed
because neither component submitted shared fill paths. After both real callers
were wired, all four cases passed. The recording seam is private/test-only and
observes successfully built paths submitted to GPUI, not synthetic gallery calls.
It checks two real independent rectangles, layer colors/pass order, clipping,
small/large widths, neutral owners and removal of stale/empty snapshots.

Tests also export SVG diagnostics from the exact command sequences submitted by
both production components. They show spectrum paths only, not the full graph
chrome, response curve, labels or handles. They are geometry review artifacts,
NOT native framebuffer captures, font/GPU proof, actual audio or visual user
acceptance. Original failure/source reports remain unchanged. Do not relabel
source-only or old native results as qualification of a later combined tree.

After source qualification, the coordinator's single Mac lane can run the new
`spectrum_painter::` cases and actual unchanged spectrum model/freshness checks
on a frozen combined candidate. No OS audio/backend/MTS/library/capture/app action
is needed for those headless fixtures. Optimized user-present review must inspect
real EQ pre/post and global master/multiple selected tracks at compact/expanded
sizes and1×/2× scaling: subtle fills, crisp distinct curves, readable overlaid
grid/labels, correct plot clipping at Nyquist/floor, no retained stale fill after
stop/selection/project changes, and unchanged node hit/Option-Q/cut-wheel input.
Use actual admitted data; no invented analyzer signal. Source merge, optimized
delivery and physical/listening acceptance remain distinct.
