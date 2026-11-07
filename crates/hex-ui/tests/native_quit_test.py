#!/usr/bin/env python3
"""Test the real GPUI/AppKit quit handshake without opening any windows.

Build first: cargo build -p hex-ui --example native_quit_probe --locked
Then run: python3 crates/hex-ui/tests/native_quit_test.py
Set CARGO_TARGET_DIR if the fixture was built into a non-default target directory.
"""

import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[3]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
PROBE = TARGET / "debug" / "examples" / "native_quit_probe"


@unittest.skipUnless(sys.platform == "darwin", "requires AppKit")
class NativeQuitTest(unittest.TestCase):
    def run_probe(self, *args):
        result = subprocess.run(
            [str(PROBE), *args], capture_output=True, text=True, timeout=3, check=True
        )
        return result.stderr

    def test_accept_quit(self):
        self.assertIn("quit request 1", self.run_probe())

    def test_cancel_then_accept_quit(self):
        output = self.run_probe("cancel-first")
        self.assertIn("quit request 1", output)
        self.assertIn("quit request 2", output)


if __name__ == "__main__":
    unittest.main()
