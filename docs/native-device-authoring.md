# native device authoring

Pulse #2670 / canonical PR285. The bootstrap kit uses existing host-independent
Lattice interfaces; it adds opt-in conformance checks, not a replacement runtime.

## minimum device contract

A device definition implements `hex_lattice_contract::NativeDeviceMetadata`:
choose one stable namespaced type ID and nonzero state version; own its bounded
codec, validation and explicit old-version migrations; declare stable parameter
IDs, defaults, canonical domains and presentation; read and patch actual state;
return validated state-aware descriptors for dynamic controls. Preserve unknown
project payloads at storage boundaries, but refuse unsupported processing.
Never encode sample/IR/table PCM inside the 4 KiB state payload. Device assets,
ports and general modulation require their W05/W08–W10 host capabilities.

An effect additionally implements `hex_lattice::NativeDeviceDefinition`. Utility
is the memoryless example (`utility.rs`); Auto Pan is a small stateful example
(`auto_pan.rs`). Prepare matrices/coefficients/storage on control. A stateful
factory declares runtime version, owned bytes per twin, algorithmic latency and
residual tail. Runtime methods process/reset/copy history without allocation,
blocking, I/O or final ownership destruction. Compatibility covers the complete
law, sample rate, layout and storage. History/parameter retargeting is explicit;
owned-history providers use the additive W04 capability described in
[owned effect transactions](owned-effect-transactions.md); large native admission
remains unfinished. Do not inflate caps
to make an example compile. Musical delay differs from algorithmic latency.

An instrument implements `NativeInstrumentDefinition` and prepares an explicit
analytic, shared-analytic or mutable capability. Sine/FM4 demonstrate analytic
notes; Vector demonstrates shared notes and mutable occurrence-aware processing.
Use real host sample offsets, occurrence IDs, note-off ownership and borrowed
musical clocks. Do not silently substitute analytic playback after mutable
preparation refuses. The generic project/storage/engine/audio layers own hosting;
ordinary device registration belongs in `hex-devices/src/lib.rs`, with no type-ID
branches in persistence, scheduling or the callback.

## standalone checks and report

```sh
cargo run -p hex-devices --example device_conformance --locked > /tmp/device-report.json
cargo test -p hex-devices --test authoring_conformance --locked
cargo test -p hex-lattice conformance --locked
```

The example and registry-discovered test need only `hex-devices`, `hex-lattice`,
neutral contracts and the standard library. They do not build a desktop, open
project files, start CPAL, initialize GPUI or access a reference device. The JSON
report includes actual type IDs, state versions/bytes, state-aware parameters,
canonical ranges/choices/default/current values, presentation and availability.
The example returns nonzero on a failed fixture rather than printing success.
Capture its source commit, compiler and command beside retained output.

External test consumers enable `hex-lattice`'s optional `conformance` feature.
Its public functions accept definitions and states through existing traits.
These helpers allocate and are **control-side test tools**, never callback code.
`hex-devices` enables the feature only as a dev dependency; ordinary desktop
production builds do not gain a conformance dependency or change DSP laws.

`check_metadata` checks codec identity/version, migration idempotence, read-value
domains, same-value edit readback, parameter availability, invalid values and
wrong-type/future-version refusal. Supply real old-state and malformed-payload
fixtures from the device's own codec tests; arbitrary bytes are not universally
invalid (the Sine payload is empty).

`check_effect` uses the actual admitted transactional chain and a deterministic
asymmetric 513-frame fixture at each explicitly requested rate. It checks finite
output and irregular partition agreement, reset reproducibility, transparent
bypass, abandoned speculative rendering and compatible-history continuation.
`check_instrument` exercises the selected current analytic/shared/mutable class
with a pitch-60 velocity-100 gate/release fixture. Mutable partition/reset checks
use the real runtime pair and explicit caller limits. The bootstrap uses existing
host limits: 2 MiB/pair, 259 event slots (256 notes plus three expression slots),
64 occurrences and a 600-second finite tail ceiling. It does not change them.

Current defaults run at 8/44.1/48/96/192 kHz. Utility's disabled filters and other
conditional/structural modes require additional authored fixture states. An
owned-pair instrument reports an explicit unsupported-harness error until its
fixture is added. Clock-dependent devices need a meaningful borrowed-clock
fixture; this initial test does not fabricate a clock or silently skip failure.
Three deliberately broken providers prove sensitivity to nonfinite reads,
invalid edits and block-dependent laws. Passing a short fixture does not prove
a full release horizon, allocation freedom, every mode or physical deadlines.
Independent transfer/event oracles and callback probes remain required.

## native panel authoring

Compose reusable `hex-widgets` with injected styles/display values and generic
gesture events; keep lowercase text/Ioskeley Mono in the Hex adapter. The current
production composition and `hex-ui`'s `layout_review` example demonstrate the
shared shell, sections, controls and constrained layouts. Run the existing
rendered gallery harness with:

```sh
cargo test -p hex-ui --example layout_review --features ui-test-support --locked
```

This existing gallery is a host-integrated example. A host-independent panel
adapter and its independently embeddable gallery remain W06 work. Preserve
canonical Begin/Change/Commit/Cancel, one-Undo behavior, preview/applied separation,
stale-owner retirement and Track/Return styling. Do not create another widget or
edit model to finish a device. Native interaction/listening belongs to the user
with an exact optimized candidate and finite checklist.

## per-device acceptance

Use [the evidence record](../research/observations/native-device-template.md).
Record all catalog features and meaningful refusal cases, stable state/migrations,
assets/presets, automation/modulation, ports/routing, musical clocks, tails/reset,
transaction failure and retirement. Retain independent expected results, exact
source/tool/fixture identity and actual measured limits. Run full local CI on the
unchanged final implementation candidate before merge.

This PR delivers the reusable bootstrap and proves it on real registered devices.
W02 remains open for two newly authored production classes using these extension
points and the independent native adapter/gallery. Those downstream proofs are
qualified incrementally under W20; they do not prevent W03–W10 enabling contracts
from proceeding against this bootstrap. No new catalog device, native acceptance,
reference parity or full authoring-platform completion is claimed here.
