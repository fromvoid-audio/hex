"""Meaningful fixture signal, WAV framing, provenance and overwrite checks."""
import hashlib
import importlib.util
import json
import struct
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("utility_audio", Path(__file__).with_name("utility_audio.py"))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class UtilityFixtureTests(unittest.TestCase):
    def test_impulse_wav_header_and_exact_channel_marker(self):
        for rate in fixture.RATES:
            data, frames = fixture.payload("impulse", rate)
            self.assertEqual(data[:4], b"RIFF")
            self.assertEqual(struct.unpack_from("<I", data, 4)[0], len(data) - 8)
            self.assertEqual(struct.unpack_from("<HHI", data, 20), (3, 2, rate))
            self.assertEqual(struct.unpack_from("<I", data, 44)[0], frames)
            self.assertEqual(data[48:52], b"data")
            pcm = data[56:]
            self.assertEqual(len(pcm), frames * 8)
            marker = rate // 4
            self.assertEqual(struct.unpack_from("<ff", pcm, marker * 8), (0.5, -0.25))
            self.assertEqual(pcm[:marker * 8], bytes(marker * 8))
            self.assertEqual(pcm[(marker + 1) * 8:], bytes((frames - marker - 1) * 8))

    def test_dc_edges_and_independent_mid_side_tones(self):
        rate = 48_000
        self.assertEqual(fixture.sample("dc-step", rate // 2 - 1, rate), (0.0, 0.0))
        self.assertEqual(fixture.sample("dc-step", rate // 2, rate), (0.25, -0.125))
        self.assertEqual(fixture.sample("dc-step", rate * 2, rate), (0.0, 0.0))
        # One quarter of the 10hz side period: left/right differ while mid tone is zero.
        left, right = fixture.sample("mid-side-tones", rate // 4 + rate // 40, rate)
        self.assertAlmostEqual((left + right) / 2, 0.0, places=12)
        self.assertAlmostEqual((left - right) / 2, 0.125, places=12)

    def test_new_directory_only_and_output_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "capture-inputs"
            manifest = fixture.generate(directory, 44_100)
            self.assertEqual(json.loads((directory / "manifest.json").read_text())["sample_rate"], 44_100)
            for source in manifest["sources"]:
                self.assertEqual(hashlib.sha256((directory / source["name"]).read_bytes()).hexdigest(), source["sha256"])
            with self.assertRaises(FileExistsError):
                fixture.generate(directory, 44_100)
            invalid = Path(temporary) / "invalid"
            with self.assertRaises(ValueError):
                fixture.generate(invalid, 0)
            self.assertFalse(invalid.exists())


if __name__ == "__main__":
    unittest.main()
