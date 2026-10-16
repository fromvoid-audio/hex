# retained import provenance

The canonical project and storage codecs preserve bounded original-document
evidence independently of musical edits. This persistence contract does not
provide a Live-set parser, CLI import command or native file-menu action. The
musical adapter remains separate work in ticket #32 / PR #150.

`ImportProvenance` retains the exact original bytes in an immutable shared owner.
The supported envelope has kind `ableton-live-set`, version `1` and adapter
version `1`, with 1–1,048,576 bytes. Unknown kinds or versions, empty input and
oversized input are refused. The model does not interpret XML, access source
paths or load plugins. Debug output reports the byte count without the contents.
Snapshots, history and normal project edits preserve this evidence unchanged;
it never represents an export of subsequent Hex edits.

Storage keeps strict readers for the recorded schema-18 and schema-19 shapes,
including the independent provenance and device-host-gain histories. Schema24
writes the complete current envelope: provenance, device host gains, Return feeds,
typed audio/MIDI routing, gain points, touch spans and recording epochs. Loading
and saving must retain all these fields together. Missing required fields,
unknown fields and unsupported versions remain errors; migration never silently
discards retained evidence. See the [storage contract](../crates/hex-storage/README.md)
for version dispatch and document limits.

The bounded provenance bytes are part of the existing 8 MiB project-document
ceiling. Serialization refuses an oversized document rather than dropping
original evidence. Create-new publication, recoverable saves and journal recovery
retain their existing safety rules. Retaining arbitrary evidence bytes does not
establish valid source XML, musical mapping, Live compatibility or listening
acceptance.
