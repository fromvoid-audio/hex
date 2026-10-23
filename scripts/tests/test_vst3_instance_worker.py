"""Owned SDK modules through the actual minimal ordinary-instance worker."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import unittest
from test_vst3_macos_bootstrap import BOOT

BINDING = (1, 2, 3, 4)
CID = bytes([0x48, 0x45, 0x58, 0x50, *range(1, 13)])


class Session:
    def __init__(self, binary, bundle, trace, *, variant=0, channels=(2, 2), maximum=256,
                 rate=48000, cid=CID, corrupt_hash=False, precision=0, config_frame=None):
        self.socket, peer = socket.socketpair()
        self.socket.settimeout(10)  # deadlock guard, not a sample/timing oracle
        env = {**os.environ, "HEX_ORIGINAL_INSTANCE_VARIANT": str(variant),
               "HEX_ORIGINAL_INSTANCE_TRACE": str(trace)}
        self.process = subprocess.Popen([str(binary)], stdin=peer, stdout=subprocess.DEVNULL,
                                        stderr=subprocess.DEVNULL, env=env)
        peer.close()
        self.sequence = self.frames = 0
        self.channels = channels
        digest = lambda p: hashlib.sha256(p.read_bytes()).digest()
        exe = bytearray(digest(bundle / "Contents/MacOS/fixture"))
        if corrupt_hash:
            exe[0] ^= 1
        self.exe_hash, self.info_hash = bytes(exe), digest(bundle / "Contents/Info.plist")
        body = struct.pack("<H4Q16s32s32sdHBBB3xII", 1, *BINDING, cid, self.exe_hash,
                           self.info_hash, rate, maximum, *channels, precision, 77, 91)
        for value in (str(bundle.resolve()), "Contents/MacOS/fixture"):
            text = value.encode()
            body += struct.pack("<I", len(text)) + text
        self.socket.sendall(config_frame if config_frame is not None else
                            b"HXVICF01" + struct.pack("<I", len(body)) + body)

    def exact(self, count):
        result = bytearray()
        while len(result) < count:
            part = self.socket.recv(count - len(result))
            if not part:
                raise AssertionError(f"early helper EOF: {len(result)}/{count}; exit={self.process.poll()}")
            result.extend(part)
        return bytes(result)

    def message(self):
        prefix = self.exact(4)
        if prefix == b"HXVB":
            header = prefix + self.exact(60)
            frames = struct.unpack_from("<H", header, 6)[0]
            if not 1 <= frames <= 256:
                raise AssertionError("invalid returned PCM extent")
            return prefix, header + self.exact(frames * 8)
        magic = prefix + self.exact(4)
        length = struct.unpack("<I", self.exact(4))[0]
        if length > 8192:
            raise AssertionError("unbounded helper control reply")
        return magic, self.exact(length)

    def packet(self, frames, *, binding=BINDING):
        return struct.pack("<4sHHH6x6Q", b"HXVB", 1, frames, 0, *binding, self.sequence, self.frames)

    def render(self, frames):
        inputs = tuple(v for n in range(frames) for v in
                       [(self.frames + n + 1) / 256 if self.channels[0] else 0,
                        -(self.frames + n + 3) / 512 if self.channels[0] == 2 else 0])
        header = self.packet(frames)
        self.socket.sendall(header + struct.pack("<" + "f" * len(inputs), *inputs))
        message = self.message()
        self.sequence += 1
        self.frames += frames
        return header, inputs, message

    def stop(self):
        self.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVE", 1, 0, 0,
                                        *BINDING, self.sequence, self.frames))
        return self.message()

    def close(self):
        self.socket.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=10)
            raise AssertionError("owned instance helper did not retire after cancellation")


@unittest.skipUnless(os.environ.get("HEX_VST3_INSTANCE_RECEIPT") and os.environ.get("HEX_VST3_MACOS_RECEIPT"),
                     "explicit minimal instance and original SDK fixture receipts required")
class NativeInstanceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.receipt = Path(os.environ["HEX_VST3_INSTANCE_RECEIPT"]).resolve()
        cls.fixtures = Path(os.environ["HEX_VST3_MACOS_RECEIPT"]).resolve()
        BOOT.verify(cls.receipt)
        BOOT.verify(cls.fixtures)
        cls.binary = cls.receipt.parent / BOOT.INSTANCE_EXECUTABLE
        cls.original = cls.fixtures.parent / BOOT.APP / "Contents/Resources/fixtures/processing.vst3"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "owned.vst3"
        shutil.copytree(self.original, self.bundle)
        self.trace = self.root / "trace.bin"

    def session(self, **kwargs):
        value = Session(self.binary, self.bundle, self.trace, **kwargs)
        self.addCleanup(value.close)
        return value

    def ready(self, session):
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIRD01", body)
        self.assertEqual(struct.unpack_from("<H4Q", body), (1, *BINDING))
        self.assertEqual(body[34:50], CID)
        self.assertEqual(body[66:68], bytes([1, 1]))  # presentation applied; component captured
        size = struct.unpack_from("<H", body, 68)[0]
        self.assertEqual(size, len(body) - 70)
        state = body[70:]
        self.assertEqual(state[:8], b"HXVS\x01\x01\x00\x00")
        self.assertEqual(state[8:24], CID)
        self.assertEqual(state[24:56], session.exe_hash)
        self.assertEqual(state[56:88], session.info_hash)
        self.assertEqual(struct.unpack_from("<II", state, 88), (32, 0))
        self.assertEqual(state[96:100], b"HXPS")
        self.assertEqual(struct.unpack_from("<IfdBB2xII", state, 100),
                         (0, 1.0, 48000.0, *session.channels, 77, 91))

    def terminal(self, session, expected, *, closed=True, unloaded=True, message=None):
        magic, body = message or session.message()
        self.assertEqual(magic, b"HXVITR01")
        values = struct.unpack("<H6QIBB", body)
        self.assertEqual(values[:5], (1, *BINDING))
        self.assertEqual(values[7:], (expected, int(closed), int(unloaded)))
        self.assertEqual(session.process.wait(timeout=10), expected)
        return values

    def error(self, session, expected):
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIER01")
        self.assertEqual(struct.unpack("<HB4QI", body), (1, 1, *BINDING, expected))

    def observation(self):
        return struct.unpack("<14i", self.trace.read_bytes())

    def test_source_selected_persistent_pcm_positions_and_negotiated_mono_stereo(self):
        for channels in [(1, 1), (1, 2), (2, 2)]:
            with self.subTest(channels=channels):
                session = self.session(channels=channels)
                self.ready(session)
                for block, frames in enumerate([1, 7, 256, 13]):
                    header, inputs, (magic, output) = session.render(frames)
                    self.assertEqual(magic, b"HXVB")
                    self.assertEqual(output[:64], header)
                    scale = .5 + block * .125
                    expected = tuple(v for n in range(frames) for v in
                                     [inputs[2*n] * scale,
                                      (inputs[2*n+1] if channels[0] == 2 else inputs[2*n]) * scale
                                      if channels[1] == 2 else 0])
                    self.assertEqual(struct.unpack("<" + "f" * (frames*2), output[64:]), expected)
                terminal = self.terminal(session, 0, message=session.stop())
                self.assertEqual(terminal[5:7], (4, 277))
                observed = self.observation()
                self.assertEqual(observed[:5], (6, 0, 1, 1, 1))
                self.assertEqual(observed[8:11], (1, 1, 4))
                self.assertEqual(observed[12:], (0, 0))

    def test_optional_presentation_and_missing_state_are_not_fabricated(self):
        shutil.rmtree(self.bundle)
        source = self.fixtures.parent / BOOT.APP / "Contents/Resources/fixtures/effect.vst3"
        shutil.copytree(source, self.bundle)
        cid = bytes.fromhex("48455833010203040506070809101100")
        session = self.session(cid=cid, maximum=64)
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIRD01", body)
        self.assertEqual(body[66:68], bytes([0, 0]))  # optional absent; missing != empty
        self.assertEqual(body[70:78], b"HXVS\x01\x00\x00\x00")
        self.assertEqual(struct.unpack_from("<II", body, 158), (0, 0))
        self.assertEqual(len(body), 166)
        header, inputs, (magic, output) = session.render(7)
        self.assertEqual((magic, output[:64]), (b"HXVB", header))
        self.assertEqual(struct.unpack("<14f", output[64:]), tuple(v*.5 for v in inputs))
        self.terminal(session, 0, message=session.stop())

    def test_plugin_stdio_cannot_corrupt_or_consume_the_owned_protocol(self):
        for variant in (21, 22):
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.ready(session)
                _header, _inputs, (magic, _body) = session.render(7)
                self.assertEqual(magic, b"HXVB")
                self.terminal(session, 0, message=session.stop())

    def test_foreign_process_faults_have_one_attempt_and_no_replay_or_fake_pcm(self):
        for variant in (4, 8, 10, 13):
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.ready(session)
                session.socket.sendall(session.packet(1) + struct.pack("<ff", .75, -.5))
                self.error(session, 7)
                self.terminal(session, 7)
                self.assertEqual(self.observation()[8:11], (1, 1, 1))

    def test_class_and_binary_refusals_never_prepare_another_instance(self):
        for change, code in [({"cid": bytes([1])*16}, 3), ({"corrupt_hash": True}, 2)]:
            with self.subTest(change=change):
                self.trace.unlink(missing_ok=True)
                session = self.session(**change)
                self.error(session, code)
                self.terminal(session, code)
                if self.trace.exists():
                    self.assertEqual(self.observation()[2:5], (0, 0, 0))

    def test_capture_presentation_start_and_teardown_refusals_are_not_ready(self):
        for variant, first, terminal, closed, unloaded in [
                (15, 6, 6, True, True), (16, 6, 10, False, False),
                (17, 5, 5, True, True), (18, 7, 7, True, True),
                (19, 2, 10, True, False), (23, 6, 6, True, True), (24, 6, 6, True, True)]:
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.error(session, first)
                self.terminal(session, terminal, closed=closed, unloaded=unloaded)
                self.assertEqual(self.observation()[10], 0)
                if not unloaded:
                    self.assertNotEqual(self.observation()[0], 6)

    def test_foreign_exception_retires_process_without_unknown_state_teardown(self):
        for variant in (25, 26):
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.error(session, 12)
                self.assertEqual(session.socket.recv(1), b"")
                self.assertEqual(session.process.wait(timeout=10), 12)
                # Startup had changed processing before throwing; getState had
                # changed its own state. Neither authorizes setActive/terminate.
                self.assertEqual(self.observation()[0], 4 if variant == 25 else 7)
                self.assertEqual(self.observation()[4], 0)

    def test_refused_module_exit_does_not_claim_unload(self):
        session = self.session(variant=20)
        self.ready(session)
        session.render(1)
        self.terminal(session, 10, unloaded=False, message=session.stop())

    def test_bad_packet_extent_and_stale_binding_refuse_before_foreign_dsp(self):
        for packet in [struct.pack("<4sHHH6x6Q", b"HXVB", 1, 257, 0, *BINDING, 0, 0),
                       struct.pack("<4sHHH6x6Q", b"HXVB", 1, 1, 0, 99, 2, 3, 4, 0, 0)]:
            with self.subTest(packet=packet[:8]):
                session = self.session()
                self.ready(session)
                session.socket.sendall(packet)
                self.error(session, 8)
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)

    def test_unsupported_configuration_refuses_before_module_entry(self):
        for config in ({"precision": 1}, {"rate": float("nan")}, {"maximum": 257},
                       {"channels": (3, 2)}, {"channels": (2, 0)}, {"cid": bytes(16)}):
            with self.subTest(config=config):
                session = self.session(**config)
                magic, body = session.message()
                self.assertEqual((magic, body), (b"HXVIER01", struct.pack("<HBI", 1, 0, 1)))
                self.assertEqual(session.process.wait(timeout=10), 1)
                self.assertFalse(self.trace.exists())

    def test_malformed_truncated_and_oversized_configuration_never_loads(self):
        for frame in [b"badmagic" + struct.pack("<I", 0),
                      b"HXVICF01" + struct.pack("<I", 8449),
                      b"HXVICF01" + struct.pack("<I", 5) + b"short",
                      b"HXVICF01" + struct.pack("<I", 50) + b"short"]:
            with self.subTest(frame=frame):
                session = self.session(config_frame=frame)
                session.socket.shutdown(socket.SHUT_WR)
                self.assertEqual(session.message(), (b"HXVIER01", struct.pack("<HBI", 1, 0, 1)))
                self.assertEqual(session.process.wait(timeout=10), 1)
                self.assertFalse(self.trace.exists())

    def test_wrong_architecture_and_linked_executable_refuse_before_entry(self):
        wrong = self.fixtures.parent / BOOT.APP / "Contents/Resources/factory-fixtures/wrong-architecture.vst3"
        binary = self.bundle / "Contents/MacOS/fixture"
        binary.write_bytes((wrong / "Contents/MacOS/fixture").read_bytes())
        session = self.session()
        self.error(session, 2)
        self.terminal(session, 2)
        self.assertFalse(self.trace.exists())
        binary.unlink()
        binary.symlink_to(self.original / "Contents/MacOS/fixture")
        session = self.session()
        self.error(session, 2)
        self.terminal(session, 2)
        self.assertFalse(self.trace.exists())

    def test_nonfinite_input_unused_lane_and_sequence_refuse_before_dsp(self):
        cases = [(self.session, 0, struct.pack("<ff", float("nan"), 0)),
                 (lambda: self.session(channels=(1, 1)), 0, struct.pack("<ff", .5, .25))]
        for create, sequence, samples in cases + [(self.session, 1, struct.pack("<ff", .25, .5))]:
            with self.subTest(sequence=sequence, samples=samples):
                session = create()
                self.ready(session)
                session.sequence = sequence
                session.socket.sendall(session.packet(1) + samples)
                self.error(session, 8)
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)

    def test_partial_pcm_is_not_a_completed_block(self):
        session = self.session()
        self.ready(session)
        session.socket.sendall(session.packet(2) + struct.pack("<f", .25))
        session.socket.shutdown(socket.SHUT_WR)
        self.error(session, 8)
        self.terminal(session, 8)
        self.assertEqual(self.observation()[10], 0)

    def test_eof_cancels_without_fabricating_graceful_retirement(self):
        session = self.session()
        self.ready(session)
        session.socket.shutdown(socket.SHUT_WR)
        self.assertEqual(session.socket.recv(1), b"")
        self.assertEqual(session.process.wait(timeout=10), 9)
        self.assertNotEqual(self.observation()[0], 6)

    def test_instance_profile_excludes_discovery_fixtures_and_generic_run_refuses(self):
        info = BOOT.verify(self.receipt)
        self.assertEqual(info["scope"], "native-macos-instance")
        self.assertIsNone(info["factory_scan_helper"])
        self.assertEqual(len(info["bundle_files"]), 4)
        with self.assertRaisesRegex(ValueError, "owned duplex"):
            BOOT.verify(self.receipt, run=True)


@unittest.skipUnless(os.environ.get("HEX_VST3_INSTANCE_RECEIPT") and os.environ.get("HEX_VST3_PACKAGE_SCANNER")
                     and os.environ.get("HEX_VST3_MACOS_RECEIPT"),
                     "explicit clean native instance, scanner and SDK fixture package inputs required")
class NativeInstancePackageTests(unittest.TestCase):
    """Owned scanner-only signed container; never installs or starts the Hex UI."""
    def setUp(self):
        from test_hex_test_build import delivery
        import plistlib
        self.delivery = delivery
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.app = self.root / "hex.app"
        contents = self.app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "Resources").mkdir()
        source = Path(os.environ["HEX_VST3_INSTANCE_RECEIPT"]).resolve()
        self.candidate = json.loads(source.read_text())["source_commit"]
        self.signing = delivery.signing_configuration(json.loads((delivery.ROOT / "scripts/hex-test-build.json").read_text()))
        shutil.copy2(os.environ["HEX_VST3_PACKAGE_SCANNER"], contents / "MacOS/hex")
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleExecutable": "hex", "CFBundleIdentifier": delivery.IDENTITY, "CFBundlePackageType": "APPL"}))
        evidence = delivery.stage_instance(self.app, source, self.candidate, self.signing)
        self.manifest = {"schema": 2, "profile": "release", "source": self.candidate,
                         "signing": self.signing, "vst3_instance": evidence}
        self.manifest["files"] = delivery.inventory(self.app, signed_payload=True)
        delivery.write_json(contents / "Resources/test-build.json", self.manifest)
        delivery.sign_bundle(self.app, self.signing)
        delivery.verify(self.app)

    def test_signed_packaged_worker_uses_exact_executable_and_owned_sdk_pcm(self):
        delivery = self.delivery
        receipt = self.app / delivery.INSTANCE_RECEIPT
        info = delivery.validate_instance_input(receipt, self.candidate, packaged=True)
        self.assertEqual(info["package_layout"], "hex-app-instance-v1")
        fixtures = Path(os.environ["HEX_VST3_MACOS_RECEIPT"]).resolve()
        bundle = fixtures.parent / BOOT.APP / "Contents/Resources/fixtures/processing.vst3"
        binary = self.app / delivery.DISCOVERY_ROOT / BOOT.INSTANCE_EXECUTABLE
        session = Session(binary, bundle, self.root / "trace")
        self.addCleanup(session.close)
        self.assertEqual(session.message()[0], b"HXVIRD01")
        header, inputs, (magic, output) = session.render(7)
        self.assertEqual((magic, output[:64]), (b"HXVB", header))
        self.assertEqual(struct.unpack("<14f", output[64:]), tuple(v*.5 for v in inputs))
        self.assertEqual(session.stop()[0], b"HXVITR01")
        self.assertEqual(session.process.wait(timeout=10), 0)

    def test_tampered_binary_or_source_does_not_verify(self):
        delivery = self.delivery
        binary = self.app / delivery.DISCOVERY_ROOT / BOOT.INSTANCE_EXECUTABLE
        original = binary.read_bytes()
        binary.write_bytes(original + b"changed")
        with self.assertRaises((delivery.DeliveryError, ValueError)):
            delivery.verify(self.app)
        binary.write_bytes(original)
        receipt = self.app / delivery.INSTANCE_RECEIPT
        data = json.loads(receipt.read_text())
        data["source_commit"] = "0" * 40
        receipt.write_text(json.dumps(data))
        with self.assertRaises((delivery.DeliveryError, ValueError)):
            delivery.verify(self.app)
