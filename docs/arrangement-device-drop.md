# Arrangement catalog device drops — pulse2333 / pr183

Registered browser instruments/effects may land on regular Arrangement lanes,
between-track gaps or empty/trailing timeline space. Lane drops address the
captured TrackId directly, independently of the selected track. Effects append
to its effect chain. An instrument fills an empty slot or explicitly replaces
the captured current instrument; it never replaces an effect or creates a second
instrument. Existing clips, effect state, track order and prior selection remain.

Gaps use the established full-width insertion line; empty space and existing
lanes show a destination outline. New-track drops retain the canonical hybrid
track defaults: an effect keeps the normal default instrument, while a browser
instrument becomes that new track's instrument. Creation and assignment form
one neutral project command, compiled once on the bounded control worker before
one history commit. One Undo restores the prior tracks/content; the existing
ID high-water rule is unchanged. No track type, schema, DSP or routing policy is
introduced. Existing rail/card device moves retain their contract.

Painted lane/gap/empty resolution is shared with PR172's OS audio adapter.
Its one-file limit, path identity, snapping, overwrite, background decode and
single import Undo are unchanged. PR172 is included through its original history
at 028a80ecbc64c18867fbb61954b5d9b99a536e62; it remains a separate open draft PR.

The release resolves fresh painted geometry and compares the captured destination
and lane bounds. Generation/revision, catalog class/type, instrument identity,
track/insertion anchor, capacities and gesture barriers are revalidated.
Main/Return targets, stale/unpainted geometry, unsupported devices and busy state
refuse visibly. Escape/outside release clears the preview and imports nothing.
Preparation failure commits no partial track, device or history operation.

Focused worker/rendered tests and exact-source full local CI are recorded in
PR183. Linux does not establish actual native macOS pointer/modifier/capture
behavior, Live reference parity, listening/hardware deadlines or optimized Mac
packaging. A fresh versioned optimized native review build and exact-source human
confirmation remain pending. The readiness/evidence hold remains in force.
