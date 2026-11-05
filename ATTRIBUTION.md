# Asset attribution

| Path | Asset | Authors | Source | License |
| --- | --- | --- | --- | --- |
| `assets/fonts/ioskeley-mono/*.ttf` | Ioskeley Mono v2.1.0, normal width, unhinted regular/medium/bold/italic | Ahmed Hatem; based on Iosevka by Renzhi Li (Belleve Invis) and contributors | [IoskeleyMono release v2.1.0](https://github.com/ahatem/IoskeleyMono/releases/tag/v2.1.0) | SIL Open Font License 1.1; retained in `assets/fonts/ioskeley-mono/OFL.txt` and embedded font metadata |

These are unmodified font files; lowercase filenames do not change the internal
family name, `Ioskeley Mono`. Source/archive and individual file SHA-256 values
are recorded in `assets/fonts/ioskeley-mono/source.json`. The font license is also
copied into the local development app bundle. Asset licensing does not choose a
license for Hex's application code.

PhotoCraft is an engineering reference. No PhotoCraft application code or assets
have been copied into this workspace.

The optional MTS-ESP adapter follows ODDSound commit
`f214739b8832e7f297cb9970d0c0efbf783f1462`; its interface notice remains in
`third_party/mts-esp-master/LICENSE-ODDSOUND.txt`. Hex does not redistribute
ODDSound's runtime library. The pinned VST3 SDK MIT/component notices are retained
by the native builder; those licenses grant no license for third-party plugin
content. See `crates/hex-plugin-scan/native/DEPENDENCIES.md` for the pinned inputs.
