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
construct fresh paired owners. Session/ViewState exports and retained waveform
geometry caches clone that same paired handle. Cache comparisons use allocation
equality only for reuse; geometry/mapping and accepted-root requirements remain
the same. Container entries account for the larger inline owner facade through
their actual element sizes. None of these operations copies PCM.

The boundary has targeted accounting, raw-ingress/mutable-escape privacy,
unknown-child and control-replacement tests. A compiled test is not an executed
test. Source-bound execution evidence is recorded in Pulse2783/2789 and PR395.
Complete parent/root traversal, old/new/preparation temporary peak, frozen
custody, atomic publication and saturated control/reaper retirement still need
their actual host proofs. Selected storage totals or alias closure do not open
referenced registry processing. Memory caps remain unchanged.
