# eq eight

Hex's native `hex.eq8` is an original registered eight-band parametric effect.
It uses the shared Lattice device contract, so project state, history, regular
track processing, Return processing, and panel bindings remain generic.

Each of five authored banks (stereo, left, right, mid, side) has eight bands.
Each band stores enabled, shape, frequency, gain, and Q. Supported shapes are
bell, low shelf, high shelf, 12 dB low cut, 48 dB low cut, 12 dB high cut,
48 dB high cut, and notch. Channel mode selects stereo-linked, independent
left/right, or mid/side banks. Output gain is stored as linear amplitude and
shown in dB. The state is an opaque versioned payload; the generic project
schema does not contain EQ-specific fields.

The DSP is an independently implemented zero-latency cascade of normalized
biquads using the public RBJ audio EQ cookbook equations. Coefficients are
prepared before publication. In 48 dB cut modes, three fixed Butterworth
sections retain their default Q and the fourth section's Q follows the authored
Q. This is an original control law, not a claim about Live's measured response.
Parameter edits ramp coefficients and output gain over 256 frames; same-rate
compatible history carries that ramp across block scratch/commit. A routing-mode
change safely resets incompatible filter history while keeping the new prepared
chain publishable. The processor reports zero algorithmic latency. IIR response
and phase delay remain inherent to the filters.

The compact panel is shared by regular tracks and Return tracks. Its left band
list selects/enables bands and chooses filter shape. The central response graph
is sampled from the prepared DSP coefficients at the project sample rate; it is
not a live analyzer. Dragging a node changes frequency and gain in one canonical
paired gesture; Option-drag changes frequency and Q, and cut/notch nodes edit Q.
Each pair is validated and committed atomically as one undoable state change.
The mode selector and channel buttons expose each bank, while compact controls
provide direct frequency, gain, Q, and output editing.

The graph uses a bounded shared widget and receives only normalized curves and
nodes. The UI translates graph movement into existing session parameter edits;
it does not own filter math or a second editing model. Generic save/load,
history, export, and audio processing use the registered device definition.

Automated source evidence includes all eight shapes, frequency response at
multiple modes, extreme Q stability at 8–192 kHz, independent channel banks,
coefficient ramp continuity across partitions, registered runtime handoff and
callback allocation probes. Native visual review, listening, detailed comparison
with the installed Live build, analyzer behavior, and exact numerical parity
remain separate native acceptance evidence; they are not prerequisites for local source CI. No Ableton code or assets are included.
