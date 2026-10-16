#!/usr/bin/env python3
"""Rebuild original WAV codec fixtures and independent FFmpeg PCM16 oracles."""
from pathlib import Path
import math
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent

def ffmpeg(*args):
    subprocess.run(['ffmpeg', '-v', 'error', '-y', *map(str, args)], check=True)

with tempfile.TemporaryDirectory() as temporary:
    raw = Path(temporary) / 'input.s16'
    for channels in (1, 2):
        samples = [int(29000 * math.sin(frame * (0.083 if channel == 0 else 0.137)))
                   for frame in range(2200) for channel in range(channels)]
        raw.write_bytes(struct.pack('<' + 'h' * len(samples), *samples))
        for codec in ('adpcm_ima_wav', 'adpcm_ms'):
            name = f'{codec}-{channels}'
            wave = ROOT / f'{name}.wav'
            ffmpeg('-f', 's16le', '-ar', 48000, '-ac', channels, '-i', raw,
                   '-map_metadata', '-1', '-c:a', codec, wave)
            decoded = ROOT / f'{name}.s16'
            ffmpeg('-i', wave, '-f', 's16le', '-c:a', 'pcm_s16le', decoded)
            # FFmpeg emits whole encoded blocks, including padding. The WAV fact
            # chunk declares the original 2200 frames; retain that logical extent.
            decoded.write_bytes(decoded.read_bytes()[:2200 * channels * 2])
    # Exhaust all 256 G.711 codewords, not merely an encoder's chosen subset.
    for encoding, name in ((6, 'alaw'), (7, 'mulaw')):
        fmt = struct.pack('<HHIIHHH', encoding, 1, 48000, 48000, 1, 8, 0)
        chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt
        chunks += b'data' + struct.pack('<I', 256) + bytes(range(256))
        wave = ROOT / f'{name}.wav'
        wave.write_bytes(b'RIFF' + struct.pack('<I', 4 + len(chunks)) + b'WAVE' + chunks)
        ffmpeg('-i', wave, '-f', 's16le', '-c:a', 'pcm_s16le', ROOT / f'{name}.s16')
