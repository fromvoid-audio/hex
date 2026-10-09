"""Create original, bit-reproducible stereo PCM16 for Session audio experiments.

Research tooling only; never part of the shipped Rust application. Refuses to
overwrite a file. Integer triangle tones and marked impulses need no source
audio, random seed, floating-point oscillator or external dependencies.
"""

import argparse
import hashlib
import json
import struct
from pathlib import Path


RATE = 48_000
FRAMES = 96_000
MARKERS = (0, 6_000, 12_000, 47_999, 48_000, 83_999, 84_000, 95_999)


def triangle(frame, period, step):
    phase = frame % period
    return (min(phase, period - phase) - period // 4) * step


def create(path):
    pcm = bytearray()
    impulses = {
        frame: (12_000 + index * 509, -18_000 + index * 367)
        for index, frame in enumerate(MARKERS)
    }
    for frame in range(FRAMES):
        sample = impulses.get(frame)
        if sample is None:
            sample = (triangle(frame, 128, 64), triangle(frame + 7, 80, 60))
            if 1_024 <= frame < 1_536:
                sample = (0, 0)
        pcm.extend(struct.pack("<hh", *sample))
    header = struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF", len(pcm) + 36, b"WAVE", b"fmt ", 16, 1, 2,
        RATE, RATE * 4, 4, 16, b"data", len(pcm),
    )
    payload = header + pcm
    with path.open("xb") as destination:
        destination.write(payload)
    return {
        "fixture": str(path.resolve()),
        "generator": "research/fixtures/session_audio.py",
        "sample_rate": RATE,
        "channels": 2,
        "encoding": "pcm16le",
        "frames": FRAMES,
        "sha256": hashlib.sha256(payload).hexdigest(),
        "pcm_sha256": hashlib.sha256(pcm).hexdigest(),
        "impulse_frames": list(MARKERS),
        "suggested_source_start": 6_000,
        "suggested_source_end_exclusive": 84_000,
        "source": "original integer-generated fixture; no third-party audio",
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    print(json.dumps(create(parser.parse_args().output), indent=2))
