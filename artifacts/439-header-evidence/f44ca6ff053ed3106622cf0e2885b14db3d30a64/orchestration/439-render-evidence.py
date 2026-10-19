import json,re,hashlib,shutil,html,subprocess
from pathlib import Path
root=Path('/workspace/hex'); source='f44ca6ff053ed3106622cf0e2885b14db3d30a64'
log=root/'artifacts/local-ci/header-width-return-439-qualified-f44ca6ff/04-real header controls and paint.log'
raw=log.read_text();line=next(x for x in raw.splitlines() if x.startswith('HEX439_RENDERED_SCENE '));scene=json.loads(line.split(' ',1)[1])
dest=root/'artifacts/439-reference-comparison';dest.mkdir(exist_ok=True)
reference=root/'artifacts/439-reference.png';assert hashlib.sha256(reference.read_bytes()).hexdigest()=='c8ac2e8ef899ae845eaac0dd6003f3a37594a84094afbe5eb90cce2080a352eb'
shutil.copy2(reference,dest/'reference.png')
for name in ['ioskeley-mono-regular.ttf','OFL.txt']:shutil.copy2(root/'assets/fonts/ioskeley-mono'/name,dest/name)
(dest/'completed-scene.json').write_text(json.dumps({'source':source,'evidence':'completed GPUI scene bounds and quad fills; captions below derived from canonical fixture values, not captured glyphs; no-op Linux shaping/no native GPU framebuffer','scene':scene},indent=2)+'\n')
byid={x['id']:x for x in scene};x0,y0=byid['track-mute-1']['bounds'][:2]
svg=['<svg xmlns="http://www.w3.org/2000/svg" width="488" height="152" viewBox="0 0 122 38"><desc>GPUI observed header-control scene subset reconstruction at 4x. Exact observed bounds and quad colors; canonical caption labels are plotted at field centers, not native glyph capture. Recording arm disabled. Not a screenshot.</desc><style>@font-face{font-family:ioskeley;src:url(ioskeley-mono-regular.ttf)} text{font-family:ioskeley,monospace;font-size:9px;text-anchor:middle;dominant-baseline:central}</style>']
for item in scene:
 bg=item['background'];x,y,w,h=item['bounds'];
 if bg and w and h:
  color=bg['solid'];assert re.fullmatch(r'#[0-9a-fA-F]{8}',color)
  svg.append(f'<rect x="{x-x0}" y="{y-y0}" width="{w}" height="{h}" rx="1" fill="{color[:7]}" fill-opacity="{int(color[7:9],16)/255:.6f}"/>')
for key,label,color in [('track-mute-1','1','#2f2c2c'),('track-solo-1','s','#d8d8d8'),('track-arm-unavailable-1','○','#777777'),('track-volume-1-value','-6.0 db','#e0e0e0'),('track-pan-1-value','c','#e0e0e0')]:
 x,y,w,h=byid[key]['bounds'];svg.append(f'<text x="{x-x0+w/2}" y="{y-y0+h/2}" fill="{color}">{html.escape(label)}</text>')
svg.append('</svg>');(dest/'completed-scene.svg').write_text('\n'.join(svg)+'\n')
old=(root/'artifacts/local-ci/header-width-return-439-baseline/01-reference composition baseline.log').read_text();failure=next(x for x in old.splitlines() if x.startswith('reference requires'));(dest/'baseline-rendered-layout.txt').write_text(failure+'\n')
(dest/'COMPARISON.md').write_text('''# source-rendered comparison for PR439

The original supplied reference bytes remain unchanged:

![user reference](reference.png)

Measured header-control subset of the completed GPUI scene, plotted at four-times scale:

![source scene reconstruction](completed-scene.svg)

This second image is a diagnostic **scene reconstruction**, not a GPU framebuffer,
native screenshot or installed acceptance. Bounds and background colors are read
from the production render through the existing GPUI test API. Captions use the
actual fixture's canonical gain0.5 (-6.0db), balance0 (c), and track number1;
their centered plotting is not evidence of native font glyph shaping or final
contrast. The bundled font is included with its OFL license for artifact viewing.
Linux uses no-op text shaping. The same reference-layout test selects native
bundled Ioskeley shaping on macOS; Root owns that check and physical comparison.

The original actual UI test failed with a86×21 volume root and a separate46×18
value at a19px horizontal center offset. Its unedited raw failure is retained in
baseline-rendered-layout.txt and the full baseline run. The new actual scene has
one60×18 root,58×16 combined well/readout with the existing one-pixel focus inset,
and centered value. Normal minimum headers remain176×44. The122×38 two-row cluster
uses62px activator,28px Solo/disabled-arm cells, and60px volume/pan fields.
Reference controls are approximately92×27 and42px buttons; the cluster keeps that
compact composition using Hex's existing palette, original scalar ranges and font.

The arm circle is disabled/unavailable (Pulse26 /0.2), and Return pan/Solo are
omitted because no such canonical fields exist. This does not complete those
functional requirements or claim Live numerical/physical parity. Native checklist
is in docs/track-header-volume.md. Full integration/review remains merger-owned.
''')
print('scene files',len(list(dest.iterdir())), 'source',source)
