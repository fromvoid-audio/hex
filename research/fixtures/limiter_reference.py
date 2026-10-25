#!/usr/bin/env python3
"""Original eight-second stereo Limiter fixture and float-WAV measurement helper.

Generate: limiter_reference.py generate NEW.wav
Measure: limiter_reference.py measure EXPORTED.wav
The experiment uses 44100 Hz, unwarped, unity clip/track gain, no return/main
processing or normalization, 32-bit float export. No proprietary assets are used.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import wave

RATE = 44100


def generate(path):
    with path.open('xb') as destination, wave.open(destination, 'wb') as output:
        output.setparams((2, 3, RATE, 0, 'NONE', 'not compressed'))
        data = bytearray()
        for n in range(RATE * 8):
            t = n / RATE
            if t < .5:
                left = right = 0
            elif t < 1:
                left = right = .75 if n % 4410 == 0 else 0
            elif t < 2:
                left = right = .8 * math.sin(2 * math.pi * 997 * t)
            elif t < 3:
                left = .8 * math.sin(2 * math.pi * 997 * t)
                right = .08 * math.sin(2 * math.pi * 997 * t)
            elif t < 4:
                left = right = .8 * math.sin(2 * math.pi * 11025 * t + math.pi / 4)
            elif t < 6:
                left = right = (.8 if t < 5 else .08) * math.sin(2 * math.pi * 997 * t)
            else:
                left = right = 0
            for sample in (left, right):
                data.extend(int(sample * 8388607).to_bytes(3, 'little', signed=True))
        output.writeframes(data)


def measure(path):
    with path.open('rb') as source:
        raw = source.read(16 * 1024 * 1024 + 1)
    if len(raw) > 16 * 1024 * 1024 or raw[:4] != b'RIFF' or raw[8:12] != b'WAVE':
        raise ValueError('expected bounded riff wav')
    offset, fmt, pcm = 12, None, None
    while offset + 8 <= len(raw):
        kind, length = struct.unpack_from('<4sI', raw, offset)
        end = offset + 8 + length
        if end > len(raw):
            raise ValueError('truncated chunk')
        chunk = raw[offset + 8:end]
        if kind == b'fmt ':
            fmt = struct.unpack_from('<HHIIHH', chunk)
        elif kind == b'data':
            pcm = chunk
        offset = end + length % 2
    if fmt != (3, 2, RATE, RATE * 8, 8, 32) or pcm is None or len(pcm) % 8:
        raise ValueError('expected stereo 44100 hz float32 wav')
    values = struct.unpack('<' + 'f' * (len(pcm) // 4), pcm)
    if not all(math.isfinite(v) for v in values):
        raise ValueError('nonfinite output')
    windows = []
    for start, end in [(0, .49), (.5, 1), (1.1, 1.9), (2.1, 2.9), (3.1, 3.9), (4.1, 4.9), (5.5, 5.9), (7, 8)]:
        segment = values[int(start * RATE) * 2:int(end * RATE) * 2]
        windows.append({'seconds': [start, end], 'peak': [max(map(abs, segment[c::2]), default=0) for c in range(2)]})
    return {'sha256': hashlib.sha256(raw).hexdigest(), 'frames': len(values) // 2, 'sample_rate': RATE, 'windows': windows}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['generate', 'measure'])
    parser.add_argument('path', type=Path)
    args = parser.parse_args()
    if args.action == 'generate':
        generate(args.path)
        print(hashlib.sha256(args.path.read_bytes()).hexdigest())
    else:
        print(json.dumps(measure(args.path), indent=2))
