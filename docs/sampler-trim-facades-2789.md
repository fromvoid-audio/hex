# retained trim metadata helpers

This partial PR395 component supplies model/Engine/Session helpers to the UI
writer. It does not admit a registered source, edit a project, construct DSP or
establish playable/native qualification.

`SnapshotClipRead` and `SnapshotMidiClipRead` expose
`content_token_capacity`, `capture_content` and `matches_content`. Read the
required capacity, end that read, then call
`ClipContentToken::reserve_on_control(capacity)`. Capture on a subsequent read
refuses insufficient capacity before changing the old token. Resize/retry only
outside a read. The token is private Copy metadata plus exact name bytes, with
no model, Arc, capability, hash, allocation identity or source witness. Capture
and comparison neither grow nor free backing. Derived equality includes stored
optional events (not the synthesized editable view), hidden intervals and next
ID, complete domain/placement/fade/warp state and the frozen tempo map's exact
sample rate, ordered points and fractional frame prefixes. MIDI includes all
ordered note fields. Allocation sharing/capacity is deliberately not equality.

Pending UI validity still requires equal project generation, revision at least
the origin, the original track and exact clip content. Audio additionally
requires the original asset's `playback_frames`; other asset metadata and the
live project tempo are not extra pending predicates. These helpers do not enforce
the caller's generation/revision/track checks. The MIDI read's
`preview_edge_trim` delegates the existing canonical Copy `MidiClipTrim` law.

For prospective audio, reserve
`hex_engine::audio_transform::WarpPreviewWorkspace::reserve_on_control()` outside
the read. Within the existing `with_audio_clip_preview` callback, call
`validate_audio_clip_preview_warp(&read, &mut workspace)`. It evaluates the actual
prospective virtual metadata without materializing a Clip/domain/marker owner.
The existing allocating renderer and this validator share the same mapping
builder, and owning/borrowed maps share constructor/stretch validation. Raw
boundaries are limited to 8192 before sort/dedup, loop cycles to 8192, source
attack counts to 1024 before discarding duplicates/out-of-source attacks, and
output to 16777216 frames. Existing pitch-compensated ratio limits, rounding and
error order remain. Geometry validity is not renderer admission; current
prepared metadata cannot certify another endpoint or interpolate valid islands.

No result cache is introduced. A caller caching results must associate them
with exact verified content and the exact operation/frame; the mutable scratch
is not a validity token. Success does not prove PCM memory/work availability,
closed-prefix/both-cap resource admission or canonical command acceptance.

Focused component tests cover exact-Eq differences, hidden fields and frozen
tempo prefixes, canonical materialization versus prospective admission, actual
limits/error precedence and zero total/net allocation/free in scoped helper
calls. Registered snapshot/UI pressure instrumentation and the completed UI
consumer integration remain separate, unexecuted checks until those fixtures
and the registered producer are available. Raw compile failures and scoped test
epochs are retained; this document is not a full-CI receipt.
