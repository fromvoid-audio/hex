# Waveform storage

Canonical shared waveforms use `hex_media::waveform::SharedWaveform`, an alias
for the restricted engine `SharedMetadata<Waveform>` facade. It consumes an owned
value through the existing sized `OwnedShared` constructor exactly once. Moves
and handle clones retain the constructor receipt beside the same shared owner.
It exposes immutable `&Waveform`, not `Arc`, `Weak`, a mutable borrow or a
detachable receipt. An owned `from_peaks` value can obtain a fresh outer owner;
an existing raw Arc cannot be adopted or upgraded. This does not qualify a
waveform's PCM children or establish resource/registry admission.

Media owns nested traversal. The existing ledger counts the qualified shared
header/body, actual leaf-bin capacity, outer level-vector capacity (including
each inline Vec header), every live level backing capacity and the actual
retained Audio or OriginalSourceLease field. Embedded source visitors do not
charge inline headers already contained in the waveform body. Distinct exported
handles remain distinct obligations; the shared body, pyramid and embedded PCM
field count once. Re-observation deduplicates actual fields, so a prior body-only
visit cannot suppress unvisited nested storage. Equal-content independent owners
remain separate. Unknown PCM remains Unknown even inside a qualified outer body.

Resident imports, batch imports, transformed-clip roots and browser previews
construct fresh paired owners. Session/ViewState exports retain that same
paired handle. The existing geometry LRU now retains only ready paths and a
nonowning immutable `SharedMetadataCacheIdentity`, not a waveform/PCM handle.
Fresh paired construction assigns a checked monotonically increasing version;
clones preserve it. Versions never wrap or reuse an address, and exhaustion
disables caching. Identity equality permits cache reuse only, never provenance,
resource admission, physical storage deduplication or source lifetime. The inline
version is charged by the enclosing facade/container's actual size. Geometry,
mapping, sampling, drag behavior and cache budgets remain unchanged. None of
these operations copies PCM.

This removes one exported owning-field family only. Paint closures, canonical
and preview snapshot capabilities, pending events and retained readers still need
the sealed snapshot-export boundary and independently bounded control retirement
specified by primary decision Pulse2783/21809. A raw `Arc<ViewState>` capture is
not sufficient. Native endpoint/device-lease retirement must not wait for GUI
readers or occupy its lifecycle lane until they release.

The boundary has targeted accounting, raw-ingress/mutable-escape privacy,
unknown-child and control-replacement tests. A compiled test is not an executed
test. Source-bound execution evidence is recorded in Pulse2783/2789 and PR395.
Complete parent/root traversal, old/new/preparation temporary peak, frozen
custody, atomic publication and saturated control/reaper retirement still need
their actual host proofs. Selected storage totals or alias closure do not open
referenced registry processing. Memory caps remain unchanged.
