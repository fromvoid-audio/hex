import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import build_device_package as builder  # noqa: E402


class DevicePackageBuilderTests(unittest.TestCase):
    def test_builds_reproducible_packages_with_provenance_and_refuses_sdk_mismatch(self) -> None:
        fixtures = builder.REPO_ROOT / "third_party" / "hex-device-abi-qual" / "fixtures"
        sdk_sha = builder.sha256_file(builder.PINNED_ABI_HEADER)

        with tempfile.TemporaryDirectory() as td:
            out_dir = Path(td)
            r1 = builder.build_package(
                c_source=fixtures / "soft_clip_v1.c",
                type_id="hex.pkg.soft_clip",
                display_name="soft_clip",
                publisher_id="hex.factory",
                device_kind=1,
                capabilities=(1 << 0) | (1 << 2) | (1 << 4) | (1 << 5),
                version=(1, 0, 0),
                state_version=1,
                ui_version=1,
                ui_schema={"controls": ["drive", "mix", "trim"]},
                notice_text="Original Hex fixture.",
                auth_token="reviewed-test-fixture-token-v1",
                reviewer_note="unit test",
                out_archive=out_dir / "a.hexdevice",
                out_receipt=out_dir / "a.receipt.json",
                expected_sdk_header_sha256=sdk_sha,
            )
            r2 = builder.build_package(
                c_source=fixtures / "soft_clip_v1.c",
                type_id="hex.pkg.soft_clip",
                display_name="soft_clip",
                publisher_id="hex.factory",
                device_kind=1,
                capabilities=(1 << 0) | (1 << 2) | (1 << 4) | (1 << 5),
                version=(1, 0, 0),
                state_version=1,
                ui_version=1,
                ui_schema={"controls": ["drive", "mix", "trim"]},
                notice_text="Original Hex fixture.",
                auth_token="reviewed-test-fixture-token-v1",
                reviewer_note="unit test",
                out_archive=out_dir / "b.hexdevice",
                out_receipt=out_dir / "b.receipt.json",
                expected_sdk_header_sha256=sdk_sha,
            )
            self.assertEqual(r1["sdk_abi_header_sha256"], sdk_sha)
            self.assertEqual(r1["source_sha256"], r2["source_sha256"])
            self.assertEqual(r1["content_digests"]["ui/panel.json"], r2["content_digests"]["ui/panel.json"])
            self.assertEqual(r1["content_digests"]["notices/NOTICE.txt"], r2["content_digests"]["notices/NOTICE.txt"])
            self.assertIn("clang", r1["compiler"].lower())
            self.assertEqual(r1["provenance_tier"], "reviewed_development_test")
            parsed_receipt = json.loads((out_dir / "a.receipt.json").read_text(encoding="utf-8"))
            self.assertEqual(parsed_receipt["type_id"], "hex.pkg.soft_clip")

            # SDK header hash mismatch must explicitly refuse.
            with self.assertRaises(ValueError):
                builder.build_package(
                    c_source=fixtures / "soft_clip_v1.c",
                    type_id="hex.pkg.soft_clip",
                    display_name="soft_clip",
                    publisher_id="hex.factory",
                    device_kind=1,
                    capabilities=1,
                    version=(1, 0, 0),
                    state_version=1,
                    ui_version=1,
                    ui_schema=None,
                    notice_text="test",
                    auth_token="tok",
                    reviewer_note="unit test",
                    out_archive=out_dir / "bad.hexdevice",
                    out_receipt=out_dir / "bad.receipt.json",
                    expected_sdk_header_sha256="0" * 64,
                )


if __name__ == "__main__":
    unittest.main()
