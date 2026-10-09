"""Generate original Utility response fixtures; research tooling, not app runtime.

Create a fresh directory with float32 stereo WAVs and a provenance manifest.
Never overwrite files. Only the two stated reference sample rates are admitted.
"""

import argparse
import hashlib
import json
import math
import struct
from pathlib import Path


RATES = (44_100, 48_000)
FREQUENCIES = (10, 25, 50, 100, 200, 500, 1000, 2000)


def sample(kind, frame, rate):
    if kind == "impulse":
        return (0.5, -0.25) if frame == rate // 4 else (0.0, 0.0)
    if kind == "dc-step":
        return (0.25, -0.125) if rate // 2 <= frame < rate * 2 else (0.0, 0.0)
    segment, local = divmod(frame, rate * 2)
    # Each segment starts from silence, leaving half a second for settling.
    if local < rate // 4 or local >= rate * 7 // 4:
        return (0.0, 0.0)
    frequency = FREQUENCIES[segment]
    phase_time = (local - rate // 4) / rate
    mid = 0.125 * math.sin(math.tau * 3000 * phase_time)
    side = 0.125 * math.sin(math.tau * frequency * phase_time)
    return (mid + side, mid - side)


def payload(kind, rate):
    frames = rate * (len(FREQUENCIES) * 2 if kind == "mid-side-tones" else 4)
    pcm = bytearray(frames * 8)
    for frame in range(frames):
        struct.pack_into("<ff", pcm, frame * 8, *sample(kind, frame, rate))
    fmt = struct.pack("<HHIIHH", 3, 2, rate, rate * 8, 8, 32)
    chunks = b"fmt " + struct.pack("<I", len(fmt)) + fmt
    chunks += b"fact" + struct.pack("<II", 4, frames)
    chunks += b"data" + struct.pack("<I", len(pcm)) + pcm
    return b"RIFF" + struct.pack("<I", len(chunks) + 4) + b"WAVE" + chunks, frames


def generate(directory, rate):
    if rate not in RATES:
        raise ValueError("reference rate must be 44100 or 48000 hz")
    directory.mkdir(parents=False, exist_ok=False)
    sources = []
    for kind in ("impulse", "dc-step", "mid-side-tones"):
        data, frames = payload(kind, rate)
        path = directory / (kind + ".wav")
        with path.open("xb") as destination:
            destination.write(data)
        sources.append({"name": path.name, "frames": frames,
                        "sha256": hashlib.sha256(data).hexdigest()})
    manifest = {
        "generator": "research/fixtures/utility_audio.py",
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "sample_rate": rate, "channels": 2, "encoding": "float32le",
        "sources": sources, "impulse_frame": rate // 4,
        "dc_start_frame": rate // 2, "dc_end_frame_exclusive": rate * 2,
        "tone_segment_frames": rate * 2, "side_frequencies_hz": FREQUENCIES,
        "mid_frequency_hz": 3000, "tone_active_start_in_segment": rate // 4,
        "tone_active_end_exclusive_in_segment": rate * 7 // 4,
        "provenance": "original generated signals; no third-party audio",
        "repeatability": "hashes identify actual outputs; sine rounding may vary by math library",
    }
    with (directory / "manifest.json").open("x") as destination:
        json.dump(manifest, destination, indent=2)
        destination.write("\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--rate", type=int, choices=RATES, default=48_000)
    arguments = parser.parse_args()
    print(json.dumps(generate(arguments.output, arguments.rate), indent=2))
