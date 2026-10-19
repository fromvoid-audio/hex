# automatic track colors

New arrangement tracks choose once from the existing red, orange, amber, olive,
green, cyan, blue, violet and rose identities. The canonical insertion helper
counts existing track colors, chooses among unused hues first and then randomly
breaks ties among the least-used hues. Palette exhaustion never refuses creation;
the existing 256-track limit still applies. Return tracks are unchanged.

The standard library's randomly seeded hasher supplies a non-security tie draw
on the control thread. There is no added random dependency, saved random state,
clock dependency, UI-side random paint or process preference mutation. The track
stores its chosen identity. Snapshot history restores that exact value on redo;
current-format persistence restores it on load. Commands rejected before commit
leave existing tracks and history unchanged. This is not a uniform-distribution
or cryptographic randomness guarantee.

The single `insert_track_before` path covers ordinary AddTrack/InsertTrack
(Command-T/menu), AddMidiTrack, explicit device-drop creation, prepared/raw
imports and audio/MIDI move/copy-to-new-track. Existing tracks retain authored
colors, including gray. Gray remains a valid persisted identity and neutral theme
role; it is absent from new-track choices and the track picker. Other hues,
clip inheritance/flavors and theme values are unchanged. Ordinary track device
contents are a separate issue (#2802), not changed here.

Tests exercise all creation families, unused/exhausted palette balance through
256 tracks, exact snapshot Undo/Redo, existing gray/red preservation, current
encode/decode stability with accent clips, and the real right-click header menu:
nine original hues/current checkmark, pointer dispatch and stale-action refusal.
The new menu fixture uses the existing constructor seam with private empty media
and preference roots, no ambient preferences or global environment mutation.
Existing clip-cache tests derive their expected color from the canonical source
track while preserving first-frame, resize and cache-reuse assertions.

Integration/full prospective local CI and merge belong to the dedicated merger.
Pulse remains In Progress while unmerged. Native/optimized delivery is separate.
Finite later user review on a coordinator-qualified build: create audio and MIDI
tracks until nine hues are used; import/drop a device; verify undo/redo and
save/reopen keep colors; right-click a track header to select a non-gray hue;
confirm existing gray tracks and inherited clip flavors are preserved. No audio
backend, permission, MTS, installed app or user-file operation is part of these
cloud source checks.
