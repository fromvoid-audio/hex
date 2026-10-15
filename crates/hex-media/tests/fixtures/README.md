# wav codec fixtures

Original generated signals, with independent decoded PCM16 oracles from FFmpeg
7.1.5 (Debian 13). Rebuild with `python3 generate.py` in this directory; the Rust
tests consume committed bytes and do not require FFmpeg or Python at runtime.

ADPCM inputs contain 2,200 frames of distinct sine signals at 48 kHz. Mono and
stereo fixtures cross block boundaries and exercise signs, step adaptation,
predictor changes, channel order and the padded final block. FFmpeg emits whole
blocks; oracle bytes are trimmed to the original frame count declared by `fact`.
G.711 fixtures enumerate every possible A-law and mu-law codeword.

These are original test assets, not recordings from a sample library or reference
application. FFmpeg is used only to generate test evidence; it is not a runtime
dependency. Decoder laws follow the Microsoft RIFF/WAVE ADPCM block structures
and IMA/DVI step/index adaptation tables; Microsoft coefficient pairs are read
from the file rather than assuming the seven common defaults.
