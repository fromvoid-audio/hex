# native device catalog and coverage

Pulse #2668 / goal 3. Audit date: 17 October 2026. This is an executable
inventory/accounting milestone, not completed device behavior or reference parity.

The [installed snapshot](../research/catalog/live-12.4.6-installed.json) pins the
actual macOS Live 12 Suite bundle to `12.4.6 (2026-09-10_0de5c8fa9a)` and executable
SHA-256 `5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
The executable hash was rechecked locally. No reference app was launched, no
project opened and no device code or factory content copied.

Reproduce the read-only file inventory with:

```sh
python3 scripts/inventory-device-catalog.py > /tmp/hex-live-device-inventory.json
```

The command reads bundle metadata, package properties, device file names/hashes
and preset-family counts. It bounds directory traversal and metadata size,
refuses symlinks and fails visibly on unreadable roots. Missing optional roots
are recorded. It neither updates the reference nor reads activation data.
Compare the result with the committed snapshot after an update; do not silently
rewrite coverage or equate a different executable/pack hash with prior evidence.

The snapshot has 22 bundled `.amxd` files and 45 pack `.amxd` files across nine
installed pack directories. Package display name, vendor, unique ID, version and
revision are retained where present. Metadata does not prove licensing, official
first-party status, browser visibility or successful loading. All discovered
entries are retained conservatively, including the Hypnus legacy overlay and
MIDI generators/transforms that are not serial devices. W19 owns their explicit
classification and native-equivalent scope; none is implicitly counted as done.

The [coverage ledger](../research/catalog/native-device-coverage.json) contains
142 stable rows: the 85 manual seeds (42 audio, eight MIDI, 13 instruments, four
racks and 18 stock Max devices), 49 additional installed identities and eight
legacy preset families. Stock Max seeds link their inspected installed IDs;
equal names in different packs remain different identities. Presets do not each
become a separate device. DS Sampler, Expression Control Legacy, Euclidean and
Velocity Shaper are among the extra bundled entries. The legacy scan also finds
Erosion Legacy and Redux Legacy. Simple Delay/Ping Pong Delay availability is
not established by that bounded preset scan and remains an open W19 audit.

Manual inventory references, inspected 17 October 2026:
[audio](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/),
[MIDI](https://www.ableton.com/en/live-manual/12/live-midi-effect-reference/),
[instruments](https://www.ableton.com/en/live-manual/12/live-instrument-reference/),
[racks](https://www.ableton.com/en/live-manual/12/instrument-drum-and-effect-racks/),
[Max devices](https://www.ableton.com/en/live-manual/12/max-for-live-devices/).
These moving pages seed identities; they do not establish installed-build laws.

## source reconciliation

The ledger's Hex source is immutable commit
`0a9a4de6bebcf73db17414daf7b8a1efd0025861`. At that source,
`crates/hex-devices/src/lib.rs` registers **seven** effects: Utility, EQ8, flexible
EQ, Saturator, Compressor, Gate and Auto Pan; and three instruments: Sine, FM4
and Vector. Flexible EQ is an additional Hex implementation associated with the
EQ coverage, not an additional accepted Live device. Sine/FM4 do not inflate
Live coverage. The borrowed-PCM sampler kernel from PR296 is present but is not
a registered or selectable instrument; see [sampler](sampler.md).

Vector is state v15, 1,728 bytes, with harmonic editing, eleven algorithms,
companion filter/envelopes/LFO, globals/controller assignments, selected
polyphony up to 32 and the original 2x quality path. Legacy patches retain their
legacy law. See [globals](vector-global.md) and [state](operator-state.md);
old 16-voice and local-control descriptions are historical. The current neutral
contract validates state-aware descriptors; the per-effect runtime-twin ceiling
is 65,536 bytes, with a separate 1,048,576-byte aggregate bank limit. Larger
instrument/runtime limits are independent. No cap is changed by this inventory.

## acceptance and remaining evidence

Each ledger row has an accountable family ticket and separate specification,
functional, native and measured-reference states. Existing source pointers and
device documents support partial original functionality only. All feature
dimensions listed in the ledger require individual audit: controls and laws,
modes, graphics, ports/events, musical context, modulation, latency/tails,
assets/presets, automation and transport/reset/bypass behavior. Unknown remains
`not-audited`; no generated title/checklist is implementation evidence.

This completes the initial installed-file accounting needed to bootstrap W02.
W01 remains open for browser-visible inventory confirmation and complete
per-feature specification/default/range/law evidence, consumed by W12–W19 and
reference owner #31. The other platform work can proceed against the audited
seed and explicit unknowns. Native user interaction/listening, pack loading,
reference captures and full device acceptance are unverified. W20 must assess
every row; member ticket states cannot close the goal automatically.
