# PhotoCraft as the engineering reference

Requested reference: [storytold/photocraft](https://github.com/storytold/photocraft).
Inspected source revision: `eec4af65513b52084926f3c679fc6ac7821cafaf` on 8 October
2026. Use it as a model for native Rust creative-app engineering, not as proof
that a complete proprietary application can be reproduced by wiring up its menus.
PhotoCraft itself distinguishes feature wiring from usable behavioral parity.
No PhotoCraft application code or assets have been copied into Hex.

| Reference pattern and source | Hex adaptation | Status |
| --- | --- | --- |
| [CommandSpec](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/crates/engine/src/commands.rs#L14-L29): stable IDs, metadata, enablement, execution, journal policy | Discrete project edits share typed commands, undo/history, UI, CLI, and future agent control | Scalar demo commands exist; general registry is planned |
| [Architecture UI seam](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/docs/architecture.md#L159-L194): commands, snapshots, events, view models | GPUI owns presentation; engine owns musical/project truth | Crate seam and atomic demo telemetry exist; editable-project snapshots are planned |
| [Layer checker](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/xtask/src/layers.rs): classified crates and toolkit dependency restrictions | Explicit goals/non-goals and a dependency check that includes GPUI, CPAL, and rtrb | Implemented in scripts/check-architecture.py |
| [Control methods](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/docs/control-protocol.md): shared command execution and inspection | Headless/agent workflows dispatch the same validated project commands | Planned; no network control surface yet |
| [Roadmap](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/docs/roadmap.md) and [scorecard](https://github.com/storytold/photocraft/blob/eec4af65513b52084926f3c679fc6ac7821cafaf/docs/scorecard.md) | Workflow/compatibility matrix with measured performance and fixture acceptance | Hand-maintained features.md and performance.md; no generated scores |

## Audio-specific differences

PhotoCraft's architecture document is a draft and describes some intended seams;
its registry/layer checker provide concrete implementation examples. Do not treat
every documented interface as already implemented or transplant its crate map.

An image editor can schedule work across a job pool and wait for results.
A DAW's callback must meet a device deadline and cannot wait, allocate, or reclaim
arbitrary media. Its control-plane command registry must compile into a separate
bounded audio-plane protocol with frame-offset events. Import, analysis, export,
and disk recording use workers; DSP state and active graph ownership remain with
the audio thread. Tile/COW imaging algorithms are not audio scheduling designs.

Adopt clear crate ownership, command-driven behavior, provenance, and measured
acceptance. Keep GPUI, lowercase text, and bundled Ioskeley Mono as Hex's explicit
product decisions. A browser build, PhotoCraft's sibling repositories, its brand,
its packaging, and its third-party asset rules are not implicit Hex dependencies.
