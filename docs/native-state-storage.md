# canonical native state storage

Pulse2783 decision21702 approves a sealed canonical owner-field family, not a
per-state receipt or a new representation. `NativeDeviceState` keeps its private
payload and optional asset-reference Arcs. Only its actual fields can produce
lifetime-bound `NativeStateStorageObservation` values. No Arc, Weak, mutable
access, raw adoption constructor or persistent address token escapes. Allocation
equality differs from physical owning-field equality. Counts establish neither
provenance nor root custody.

The production constructors are closed: nonempty `Vec<u8>` to `Arc<[u8]>`,
nonempty `Vec<DeviceAssetReference>` to its Arc slice, and fresh `Arc::new([u8;0])`
followed by unsizing for an empty payload. Absent references allocate nothing.
Clone and host-metadata restoration clone existing fields; no inner COW exists.
Reference elements contain scalar slot/key values. Adding raw/custom allocator,
COW, mutable/Weak ingress or an alternate deserializer path requires fresh
qualification or Unknown. The lattice contract has no engine dependency.

The separately named engine gate is
`canonical-native-state-vec-slices-empty-array-v1`. It shares environmental
rejection with the pinned std selector, not the PCM constructor-family label.
It supports only ordinary native Rust1.94.0 commit
`4a4ef493e3a1488c6e321570238084b38948f6db`, x86_64 Linux GNU or arm64 macOS.
Unsupported compiler/install/wrapper/flags/target or missing evidence refuses.
Compiler selection is not allocator attestation, RSS or usable-size measurement.

Independently read primary source:
[pinned alloc/src/sync.rs](https://github.com/rust-lang/rust/blob/4a4ef493e3a1488c6e321570238084b38948f6db/library/alloc/src/sync.rs),
SHA256 `ce15d00d92133cf1f55cd1905d7fe6b4e23e62090dabff6c68fae2d7fe78fe8f`.
`ArcInner`383 has two atomic counters. Layout helper395 extends that header with
the element-array layout, then pads to alignment. Sized new419 allocates the
empty array's header/body; unsizing preserves that allocation. Vec conversion4007
allocates a NEW slice for len, moves elements and deallocates the old Vec buffer
using its original capacity. Slice allocation uses the same checked header/body
layout. The engine computes this requested layout with checked arithmetic, not
a universal `16 + len` rule.

Selected traversal reuses existing DevicePayload/DeviceAssetReferences domains
and inline-state byte/field locations. Each body/header is charged once, including
the fresh empty payload allocation. Every actual field alias is visited; repeated
visits cannot substitute for an omitted alias. Inline NativeDeviceState and its
Arc fields belong to enclosing object/container storage once. Actual Project
track/return effect and instrument fields and current/Undo/Redo states are borrowed
without making synthetic clones. Candidates join through their actual Project.
Prepared context copies remain separate actual owners, never substitutes for
model owners. Existing qualified Utility payload observations use the same byte
and field locations so they cannot create a second charge for that allocation.

This is selected storage only. Complete prepared/pending/outgoing graph traversal,
enclosing model bodies/containers/other children, frozen root custody, atomic
publication and saturated reaper final-drop still require proof. Registry resource
processing remains closed. In particular final Arc observations cannot recover
input Vec capacity or historical construction peak. Admitted construction must
quote old/new/input/history/temporary overlap BEFORE allocation, including an
empty overreserved input Vec and failures. No historical-alpha migration applies.
