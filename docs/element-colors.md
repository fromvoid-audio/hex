# element colors

The shared palette has ten identities: gray, red, orange, amber, olive, green,
cyan, blue, violet and rose. `hex-project::ElementColor` stores the identity;
`hex-ui::theme::element_colors` resolves restrained theme-coherent RGB values and
contrast text. The existing pending collection markers already use this resolver; saved
collections and their editing remain pending. Other element markers can reuse it. There is no arbitrary per-clip RGB picker.

Each hybrid track owns one identity (default gray). All Arrangement and Session
audio/MIDI clips inherit their current track's color. Each clip independently
stores `ClipFlavor::Muted` or `Accent`; accent is the brighter version of the
same hue. New clips default to muted. Moving between tracks adopts the destination
hue while retaining the flavor. Copy, duplicate, split, trim, range edits and
source-kind conversion preserve flavor. Playback data and timing are unaffected.

Right-click an Arrangement or Session track header to choose a checked color
swatch. The existing insert-track action remains available. Right-click a clip
body or populated Session cell to switch its flavor. Empty lane/arranger menus
retain insertion behavior. Menus support arrows, Enter, Escape and outside-click
dismissal, restore focus and bind generation/stable owner IDs so stale targets
cannot recolor another project or a moved clip.

`SetTrackColor` and `SetClipFlavor` use canonical transactional editing and undo/
redo. Schema27 retains the complete schema26 envelope plus a complete bounded
track-color table and a bounded sparse accented-clip list. Missing/duplicate/
unknown track colors and duplicate/non-clip accent references are refused.
Schemas1–26 default to gray/muted. Explicit old-format encoders refuse appearance
loss. Snapshot constructors across tests/adapters explicitly supply these defaults.

Native macOS acceptance remains pending: change a header color and check all its
clips; toggle one clip twice; move/copy it to a differently colored track; check
Arrangement and Session audio/MIDI, narrow clips and selected states; Undo/Redo;
save/reopen; Escape/outside dismissal and editor shortcut focus restoration.
Automated source checks do not establish physical input or native pixel fidelity.
