"""Installed inventory must preserve overlays and refuse incomplete traversal."""
import importlib.util
import json
import plistlib
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    'device_catalog', Path(__file__).resolve().parents[1] / 'inventory-device-catalog.py')
CATALOG = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CATALOG)


class DeviceInventoryTests(unittest.TestCase):
    def test_overlay_identity_legacy_and_missing_root_are_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            app = base / 'Live.app'
            contents = app / 'Contents'
            (contents / 'MacOS').mkdir(parents=True)
            (contents / 'MacOS/Live').write_bytes(b'original fixture executable')
            (contents / 'Info.plist').write_bytes(plistlib.dumps({
                'CFBundleExecutable': 'Live', 'CFBundleVersion': 'fixture',
                'CFBundleShortVersionString': 'fixture', 'CFBundleIdentifier': 'fixture.live'}))
            resources = contents / 'App-Resources'
            core = resources / 'Core Library/Devices/Audio Effects/Legacy/Chorus'
            core.mkdir(parents=True)
            (core / 'preset.adv').write_bytes(b'not opened or executed')
            packs = base / 'packs'
            for pack in ['A', 'B']:
                folder = packs / pack / 'Ableton Folder Info'
                folder.mkdir(parents=True)
                (folder / 'Same.amxd').write_bytes(pack.encode())
                (folder / 'Properties.cfg').write_text(
                    'String PackVendor = "Example"; Int PackRevision = 7;')
            result = CATALOG.inventory(app, packs)
            self.assertEqual(result['roots']['builtin']['availability'], 'missing')
            self.assertEqual(len({x['id'] for x in result['max_devices']}), 2)
            self.assertEqual(len({x['sha256'] for x in result['max_devices']}), 2)
            self.assertEqual(result['native_preset_groups'][0]['path'],
                             'Audio Effects/Legacy/Chorus')
            self.assertEqual(result['packs'][0]['metadata']['values']['PackRevision'], 7)
            json.dumps(result)

    def test_symlink_and_oversized_metadata_refuse_instead_of_silent_omission(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'loop').symlink_to(root, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'symlink'):
                list(CATALOG.files(root))
            path = root / 'metadata'
            path.write_bytes(b'x' * (CATALOG.MAX_METADATA_BYTES + 1))
            with self.assertRaisesRegex(ValueError, 'oversized'):
                CATALOG.metadata(path)


if __name__ == '__main__':
    unittest.main()
