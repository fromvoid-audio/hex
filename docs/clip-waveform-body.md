# Arrangement waveform body

Pulse2848 / PR450 keeps audio waveform canvas geometry and its paint mask below
`CLIP_NAME_HEIGHT`. Regular clips, captured trim/stretch overlays and audio drag
ghosts use a body-only clipping container. Internal clip ghosts now share the canonical name-strip height under PR492;
external asset placeholders retain their existing bounded title treatment. Source/time projection, waveform preparation, peak selection, fades,
clip bounds, outer selection border and hit targets are unchanged.

Captured audio trim/stretch previews now retain the same title strip through a
read-only composition of the canonical name surface. It installs no click/rename
handler over drag capture. The outer clip continues to own selection and rounded
clipping; the body introduces no new fill, border or radius. A row with no body
space paints no waveform, rather than shrinking it into the title.

The initial actual painter regression observed title bottom248.5px while the
waveform canvas/mask began235px and path ordinates243.19..245.5px occupied the
title. Fixed normal/selected/looping and normal/Shift edge-preview cases measure
the actual title bounds and require waveform masks to exclude them. Preview cases
also release through the real offline Service and verify canonical content plus
one Undo. Header-only/tiny-body cases use the same production container and actual
waveform canvas, deterministic bipolar peaks and actual pixel-rounded title bounds.
These are GPUI scene/painter observations, not native GPU pixels or listening.

Integration: PR432's pending hue/foreground/cached lookup belongs in the shared
`clip_name_strip` styling implementation, not only its interactive wrapper. This
change deliberately does not import that unmerged feature. Future loop-rounded
bottoms and minimum-height tracks must keep their parent mask and common header
height; no separate waveform title height is introduced.

## Finite native request

On the exact frozen source, in the coordinator's private target/cache, run:

```sh
cargo test -p hex-ui --features ui-test-support --lib editor::waveform::waveform_header_ui_test:: -- --nocapture
```

Expect exactly3 unique tests, zero ignored: ordinary/selected/looped actual
painter; normal/Shift both-edge preview+worker/Undo; header-only/tiny body actual
canvas and paint mask. Fixtures create and retain unique disposable browser,
samples/recent/theme paths before constructor workers. The canvas-only case has
no EditorView or filesystem worker. No app launch, OS audio, permissions or
physical input is required. A narrow headless pass does not qualify the merger's
future tree or optimized app.

User-present optimized observation remains open: inspect selected/unselected,
normal/faded/warped/looping clips at compact and header-only heights, scroll/zoom
across clipped edges, drag both edges while held then release, and confirm readable
colored titles, all four selection edges and rounded loop bottoms after their
respective feature integration. Check name click/rename/header-grab and Undo.
Root owns native scheduling and app delivery; this source change performs neither.
