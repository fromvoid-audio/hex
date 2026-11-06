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
import time
import unittest
from test_vst3_macos_bootstrap import BOOT

BINDING = (1, 2, 3, 4)
CID = bytes([0x48, 0x45, 0x58, 0x50, *range(1, 13)])


class Session:
    def __init__(self, binary, bundle, trace, *, variant=0, channels=(2, 2), maximum=256,
                 rate=48000, cid=CID, corrupt_hash=False, precision=0, config_frame=None, auto_present=True, startup_state=None, startup_frame=None, tail_frames=None):
        self.socket, peer = socket.socketpair()
        # Fresh copied macOS modules can spend over10s in dyld's code-signature
        # validation before their first reply (see native test failure samples).
        # This is a loader deadlock guard, never a plugin/callback deadline.
        self.socket.settimeout(60)
        env = {**os.environ, "HEX_ORIGINAL_INSTANCE_VARIANT": str(variant),
               "HEX_ORIGINAL_INSTANCE_TRACE": str(trace)}
        self.tail_frames = tail_frames if tail_frames is not None else 0
        if tail_frames is not None:
            env["HEX_ORIGINAL_TAIL_FRAMES"] = str(tail_frames)
        if variant >= 37:
            env["HEX_ORIGINAL_RESTORE_TRACE"] = str(trace) + "-restore"
        self.process = subprocess.Popen([str(binary)], stdin=peer, stdout=subprocess.DEVNULL,
                                        stderr=subprocess.DEVNULL, env=env)
        peer.close()
        self.sequence = self.frames = 0
        self.channels = channels
        self.auto_present = auto_present
        digest = lambda p: hashlib.sha256(p.read_bytes()).digest()
        exe = bytearray(digest(bundle / "Contents/MacOS/fixture"))
        if corrupt_hash:
            exe[0] ^= 1
        self.exe_hash, self.info_hash = bytes(exe), digest(bundle / "Contents/Info.plist")
        body = struct.pack("<H4Q16s32s32sdHBBB3x", 1, *BINDING, cid, self.exe_hash,
                           self.info_hash, rate, maximum, *channels, precision)
        for value in (str(bundle.resolve()), "Contents/MacOS/fixture"):
            text = value.encode()
            body += struct.pack("<I", len(text)) + text
        self.facts_prefix = body[:126]
        self.restore_suffix = struct.pack("<BH", startup_state is not None, len(startup_state or b"")) + (startup_state or b"")
        startup = struct.pack("<H4Q", 1, *BINDING) + self.restore_suffix
        request = config_frame if config_frame is not None else b"HXVICF03" + struct.pack("<I", len(body)) + body
        state_request = startup_frame if startup_frame is not None else b"HXVIST01" + struct.pack("<I", len(startup)) + startup
        self.socket.sendall(request + state_request)

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
            version, parameters = struct.unpack_from("<H", header, 4)[0], struct.unpack_from("<H", header, 10)[0]
            if not 1 <= frames <= 256:
                raise AssertionError("invalid returned PCM extent")
            if version not in (1, 2) or parameters > 64 or (version == 1 and parameters) or any(header[12:16]):
                raise AssertionError("invalid returned parameter extent")
            return prefix, header + self.exact(frames * 8 + parameters * 16)
        magic = prefix + self.exact(4)
        length = struct.unpack("<I", self.exact(4))[0]
        if length > 8192:
            raise AssertionError("unbounded helper control reply")
        body = self.exact(length)
        # Loading has replied. Keep all processing/capture/retirement protocol
        # waits at their existing bound; only first-response startup differs.
        self.socket.settimeout(10)
        if magic == b"HXVIFA03" and self.auto_present:
            if len(body) != 135 + len(self.restore_suffix) or body[:126] != self.facts_prefix or body[134] not in (0, 1) or body[135:] != self.restore_suffix:
                raise AssertionError("wrong source/configuration preparation facts")
            if struct.unpack_from("<I", body, 130)[0] != self.tail_frames:
                raise AssertionError("wrong source tail report")
            self.auto_present = False
            self.presentation((77, 91))
            return self.message()
        return magic, body

    def presentation(self, values, binding=BINDING, *, allow_unknown=False):
        mode = 2 if allow_unknown else int(values is not None)
        body = struct.pack("<H4QB", 1, *binding, mode)
        if values is not None:
            body += struct.pack("<II", *values)
        self.socket.sendall(b"HXVIPR01" + struct.pack("<I", len(body)) + body)

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

    def render_parameters(self, frames, points):
        inputs = tuple(v for n in range(frames) for v in
                       [(self.frames + n + 1) / 256 if self.channels[0] else 0,
                        -(self.frames + n + 3) / 512 if self.channels[0] == 2 else 0])
        header = struct.pack("<4sHHHH4x6Q", b"HXVB", 2, frames, 0, len(points),
                             *BINDING, self.sequence, self.frames)
        payload = b"".join(struct.pack("<HHId", offset, 0, parameter, value) for offset, parameter, value in points)
        self.socket.sendall(header + struct.pack("<" + "f" * len(inputs), *inputs) + payload)
        message = self.message()
        self.sequence += 1
        self.frames += frames
        return inputs, message

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

    def ready(self, session, *, controller=False):
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIRD04", body)
        self.assertEqual(struct.unpack_from("<H4Q", body), (1, *BINDING))
        self.assertEqual(body[34:50], CID)
        self.assertEqual(struct.unpack_from("<I", body, 66)[0], session.tail_frames)
        self.assertEqual(body[70:72], bytes([1, 1]))  # applied; known contributions echoed
        self.assertEqual(struct.unpack_from("<II", body, 72), (77, 91))
        self.assertEqual(body[80:82], bytes([bool(session.restore_suffix[0]), 1]))
        size = struct.unpack_from("<H", body, 82)[0]
        self.assertEqual(size, len(body) - 84)
        state = body[84:]
        self.assertEqual(state[:8], b"HXVS\x01" + bytes([3 if controller else 1, 0, 0]))
        self.assertEqual(state[8:24], CID)
        self.assertEqual(state[24:56], session.exe_hash)
        self.assertEqual(state[56:88], session.info_hash)
        self.assertEqual(struct.unpack_from("<II", state, 88), (32, 8 if controller else 0))
        self.assertEqual(state[96:100], b"HXPS")
        self.assertEqual(struct.unpack_from("<IfdBB2xII", state, 100),
                         (0, 1.0, 48000.0, *session.channels, 77, 91))
        if controller:
            self.assertEqual(state[128:], b"HXCT" + struct.pack("<I", 7))

    def test_actual_parameter_step_ramp_and_feedback_across_variable_sdk_blocks(self):
        session = self.session(variant=37)
        self.ready(session, controller=True)
        cases = [
            (1, [(0, 7, .25)], [.25]),
            (7, [(6, 7, .75)], [.25 + .5 * (n + 1) / 7 for n in range(7)]),
            (64, [(0, 7, .75), (30, 7, .75), (31, 7, .125), (63, 7, .125)],
             [.75 if n < 31 else .125 for n in range(64)]),
            (256, [(255, 7, 1.0)], [.125 + .875 * (n + 1) / 256 for n in range(256)]),
        ]
        for block, (frames, points, curve) in enumerate(cases):
            with self.subTest(frames=frames):
                inputs, (magic, reply) = session.render_parameters(frames, points)
                self.assertEqual(magic, b"HXVB")
                self.assertEqual(struct.unpack_from("<HHHH", reply, 4), (2, frames, 0, 1))
                output = struct.unpack_from("<" + "f" * frames * 2, reply, 64)
                scale = .5 + block * .125
                for frame, normalized in enumerate(curve):
                    for channel in (0, 1):
                        self.assertAlmostEqual(output[frame * 2 + channel],
                                               inputs[frame * 2 + channel] * scale * normalized * 4,
                                               places=6)
                self.assertEqual(struct.unpack_from("<HHId", reply, 64 + frames * 8),
                                 (frames - 1, 0, 7, curve[-1]))
                self.assertEqual(len(reply), 64 + frames * 8 + 16)
        # Empty queues retain the last actual DSP value; they do not reset it
        # or echo the preceding block's feedback as a new processor change.
        header, inputs, (magic, reply) = session.render(13)
        self.assertEqual(magic, b"HXVB")
        self.assertEqual(reply[:64], header)
        self.assertEqual(struct.unpack_from("<26f", reply, 64), tuple(value * 4 for value in inputs))
        self.terminal(session, 0, message=session.stop())

    def test_editor_unavailable_completes_without_stopping_original_pcm(self):
        session = self.session(variant=37)
        self.ready(session, controller=True)

        def exchange(magic, operation=1, action=6):
            session.socket.sendall(struct.pack("<4sHHH6x6QQHQBIQ256x", magic, 1, 0, 0,
                                               *BINDING, session.sequence, session.frames,
                                               operation, 0, 0, action, 0, 0))
            reply_magic, reply = session.message()
            self.assertEqual(reply_magic, b"HXVIMD01")
            self.assertEqual(struct.unpack_from("<H7Q", reply),
                             (1, *BINDING, session.sequence, session.frames, operation))
            return reply[58:]

        def terminal(operation, action):
            until = time.monotonic() + 2
            while (reply := exchange(b"HXVN", operation, action)) == b"\0":
                self.assertLess(time.monotonic(), until)
                time.sleep(.001)
            return reply

        self.assertEqual(exchange(b"HXVM"), b"\0")
        self.assertEqual(session.render(7)[2][0], b"HXVB")
        self.assertEqual(terminal(1, 6), b"\x04\x02")
        self.assertEqual(exchange(b"HXVM", 2, 7), b"\0")
        self.assertEqual(terminal(2, 7), b"\x04\0")
        self.assertEqual(session.render(7)[2][0], b"HXVB")
        self.assertEqual(self.terminal(session, 0, message=session.stop())[5:7], (2, 14))

    def instrument_block(self, session, notes, frames=8):
        header = bytearray(session.packet(frames))
        struct.pack_into("<H", header, 8, len(notes))
        events = b"".join(struct.pack("<HBBBB2x", offset, 3, status, key, velocity)
                          for offset, status, key, velocity in notes)
        session.socket.sendall(header + bytes(frames * 8) + events)
        reply = session.message()
        session.sequence += 1
        session.frames += frames
        return reply

    def test_instrument_note_offsets_gate_pcm_across_blocks(self):
        for variant in (115, 120):
            with self.subTest(variant=variant):
                session = self.session(variant=variant, channels=(0, 2))
                self.assertEqual(session.message()[0], b"HXVIRD04")
                for notes, expected in [
                    ([(3, 0x92, 60, 127)], [0.] * 3 + [.5] * 5),
                    ([(4, 0x92, 60, 0)], [.625] * 4 + [0.] * 4),
                    ([], [0.] * 8),
                ]:
                    magic, reply = self.instrument_block(session, notes)
                    self.assertEqual(magic, b"HXVB")
                    self.assertEqual(reply[8:10], bytes(2))
                    self.assertEqual(struct.unpack("<16f", reply[64:]), tuple(x for v in expected for x in (v, v)))
                self.assertEqual(self.terminal(session, 0, message=session.stop())[5:7], (3, 24))

    def test_silent_input_instrument_refuses_routed_pcm_and_requires_a_midi_bus(self):
        audio_only = self.session(variant=0, channels=(0, 2))
        self.error(audio_only, 20)
        self.terminal(audio_only, 20)

        session = self.session(variant=120, channels=(0, 2))
        self.assertEqual(session.message()[0], b"HXVIRD04")
        session.socket.sendall(session.packet(8) + struct.pack("<16f", 1., *([0.] * 15)))
        self.error(session, 8)
        self.terminal(session, 8)
        self.assertEqual(self.observation()[10], 0)

    def test_out_of_range_note_identity_refuses_before_dsp(self):
        session = self.session(variant=115, channels=(0, 2))
        self.assertEqual(session.message()[0], b"HXVIRD04")
        header = bytearray(session.packet(8))
        struct.pack_into("<HHH", header, 4, 3, 8, 1)
        event = struct.pack("<HBBBB2xI", 0, 3, 0x92, 60, 127, 2**31)
        session.socket.sendall(header + bytes(64) + event)
        self.error(session, 8)
        self.terminal(session, 8)
        self.assertEqual(self.observation()[10], 0)

    def test_midi_outside_the_active_instrument_bus_refuses_before_dsp(self):
        for channels, variant, note in [
            ((0, 2), 115, (0, 0x94, 60, 127)),  # the fixture has four MIDI channels
            ((0, 2), 115, (0, 0xb2, 60, 127)),  # controllers are not note events
            ((2, 2), 0, (0, 0x92, 60, 127)),
        ]:
            with self.subTest(channels=channels, note=note):
                session = self.session(variant=variant, channels=channels)
                self.assertEqual(session.message()[0], b"HXVIRD04")
                magic, error = self.instrument_block(session, [note])
                self.assertEqual(magic, b"HXVIER01")
                self.assertEqual(struct.unpack("<HB4QI", error), (1, 1, *BINDING, 8))
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)

    def test_processor_feedback_reaches_original_controller_before_live_and_quiescent_capture(self):
        session = self.session(variant=37)
        self.ready(session, controller=True)
        for n in range(32):
            # Rapid changes may coalesce for display, but the final value must
            # reach the real controller rather than just its copied Rust reply.
            _, (magic, _) = session.render_parameters(7, [(6, 7, (n + 1) / 64)])
            self.assertEqual(magic, b"HXVB")
        def capture(magic, operation):
            session.socket.sendall(struct.pack("<4sHHH6x7Q", magic, 1, 0, 0,
                                              *BINDING, session.sequence, session.frames, operation))
            return session.message()
        magic, body = capture(b"HXVL", 1)
        self.assertEqual(magic, b"HXVLCA01")
        for _ in range(10000):
            magic, body = capture(b"HXVP", 1)
            self.assertEqual(magic, b"HXVLCA01")
            if body[58]:
                break
        else:
            self.fail("actual controller feedback/capture did not complete")
        state = body[61:]
        self.assertEqual(struct.unpack_from("<f", state, 104)[0], 2.0)
        self.assertEqual(self.restore_observation()[6], struct.unpack("<i", struct.pack("<f", 2.0))[0])
        _, (magic, _) = session.render_parameters(1, [(0, 7, .875)])
        self.assertEqual(magic, b"HXVB")
        session.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVQ", 1, 0, 0,
                                          *BINDING, session.sequence, session.frames))
        self.assertEqual(session.message()[0], b"HXVIQF02")
        self.assertEqual(self.restore_observation()[6], struct.unpack("<i", struct.pack("<f", 3.5))[0])
        magic, body = capture(b"HXVC", 1)
        self.assertEqual(magic, b"HXVICS01")
        self.assertEqual(struct.unpack_from("<f", body, 60 + 104)[0], 3.5)
        self.terminal(session, 0, message=session.stop())
        self.assertEqual(self.observation()[1], 0, "controller setters stay on main")

    def test_stopped_flush_rejects_live_stale_malformed_and_unknown_before_sdk_entry(self):
        for mode in ("live", "operation", "prefix", "binding", "offset", "unknown", "duplicate"):
            with self.subTest(mode=mode):
                session=self.session(variant=37);self.ready(session, controller=True)
                if mode!="live":
                    session.socket.sendall(struct.pack("<4sHHH6x6Q",b"HXVQ",1,0,0,*BINDING,0,0))
                    self.assertEqual(session.message()[0],b"HXVIQF02")
                binding=(*BINDING[:3],99) if mode=="binding" else BINDING
                operation=2 if mode=="operation" else 1
                prefix=1 if mode=="prefix" else 0
                packet=struct.pack("<4sHHHH4x7QHHId",b"HXVF",2,0,0,1,*binding,prefix,0,operation,
                                   1 if mode=="offset" else 0,0,999 if mode=="unknown" else 7,.5)
                session.socket.sendall(packet)
                if mode=="duplicate":
                    magic,body=session.message();self.assertEqual(magic,b"HXVIFL01")
                    self.assertEqual(struct.unpack_from("<H7QBHId",body),(1,*BINDING,0,0,1,1,1,7,.5))
                    session.socket.sendall(packet)
                self.error(session,8);self.terminal(session,8)
                self.assertEqual(self.observation()[10],0)
                self.assertEqual(self.observation()[11],1 if mode=="duplicate" else 0)

    def test_parameter_unknown_ids_offsets_and_nonfinite_values_refuse_before_sdk_processing(self):
        for point in [(0, 9999, .5), (8, 7, .5), (0, 7, float("nan")), (0, 0x80000000, .5)]:
            with self.subTest(point=point):
                session = self.session(variant=37)
                self.ready(session, controller=True)
                _, (magic, body) = session.render_parameters(8, [point])
                self.assertEqual(magic, b"HXVIER01")
                self.assertEqual(struct.unpack("<HB4QI", body), (1, 1, *BINDING, 8))
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)

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

    def startup_state(self, *, component=True, controller=True, gain=.75, latency=3, scroll=91):
        comp = struct.pack("<4sIfdBB2xII", b"HXPS", latency, gain, 48000., 2, 2, 0, 0) if component else b""
        ctrl = struct.pack("<4sI", b"HXCT", scroll) if controller else b""
        digest = lambda path: hashlib.sha256(path.read_bytes()).digest()
        header = struct.pack("<4sBB2x16s32s32sII", b"HXVS", 1, int(component) | (int(controller) << 1),
                             CID, digest(self.bundle / "Contents/MacOS/fixture"),
                             digest(self.bundle / "Contents/Info.plist"), len(comp), len(ctrl))
        return header + comp + ctrl

    def restore_observation(self):
        return struct.unpack("<15i", Path(str(self.trace) + "-restore").read_bytes())

    def test_startup_restore_precedes_facts_and_preserves_component_controller_and_pcm(self):
        captured = self.startup_state()
        session = self.session(variant=37, auto_present=False, startup_state=captured)
        magic, facts = session.message()
        self.assertEqual(magic, b"HXVIFA03")
        self.assertEqual(struct.unpack_from("<IIB", facts, 126), (3, 0, 1))
        self.assertEqual(facts[135:], session.restore_suffix)
        before = self.restore_observation()
        self.assertEqual(before[:3], (1, 1, 1))
        self.assertGreaterEqual(before[3], 1)
        self.assertGreaterEqual(before[4], 1)
        self.assertEqual(before[5:10], (struct.unpack("<i", struct.pack("<f", .75))[0],
                                      struct.unpack("<i", struct.pack("<f", .75))[0], 91, 3, 3))
        self.assertEqual(before[10:12], (0, 0), "startup restore is not a premature state capture")
        self.assertEqual((self.observation()[8], self.observation()[10]), (0, 0))
        session.presentation((77, 91))
        magic, ready = session.message()
        self.assertEqual(magic, b"HXVIRD04")
        self.assertEqual(ready[80:82], b"\x01\x01")
        state = ready[84:]
        self.assertEqual(struct.unpack_from("<II", state, 88), (32, 8))
        self.assertEqual(struct.unpack_from("<If", state, 100), (3, .75))
        self.assertEqual(state[128:], struct.pack("<4sI", b"HXCT", 91))
        self.assertEqual(struct.unpack_from("<II", state, 120), (77, 91))
        self.assertNotEqual(state, captured, "actual presentation capture may differ; caller state is retained separately")
        self.assertEqual(self.restore_observation()[10:12], (1, 1))
        generated, actual = [], []
        for block in range(3):
            _, inputs, (magic, output) = session.render(8)
            self.assertEqual(magic, b"HXVB")
            generated.extend(value * (.5 + block * .125) * .75 for value in inputs)
            actual.extend(struct.unpack("<16f", output[64:]))
        self.assertEqual(actual, [0.] * 6 + generated[:-6], "reported three-frame latency is real DSP delay")
        self.terminal(session, 0, message=session.stop())
        self.assertEqual(self.observation()[1], 0)
        self.assertEqual(self.restore_observation()[12:15], (1, 1, 1))

    def test_fresh_controller_sync_uses_the_one_capture_after_actual_presentation(self):
        session = self.session(variant=37, auto_present=False)
        self.assertEqual(session.message()[0], b"HXVIFA03")
        self.assertEqual(self.restore_observation()[:3], (0, 0, 0))
        self.assertEqual(self.restore_observation()[10:12], (0, 0))
        session.presentation((77, 91))
        magic, ready = session.message()
        self.assertEqual(magic, b"HXVIRD04")
        self.assertEqual(ready[80], 0)
        observed = self.restore_observation()
        self.assertEqual(observed[:3], (0, 1, 0))
        self.assertEqual(observed[10:12], (1, 1))
        self.assertEqual(observed[6], struct.unpack("<i", struct.pack("<f", 1.))[0])
        self.terminal(session, 0, message=session.stop())

    def test_unimplemented_controller_sync_preserves_both_chunks_and_restored_pcm(self):
        fresh = self.session(variant=117)
        self.ready(fresh, controller=True)
        self.terminal(fresh, 0, message=fresh.stop())

        state = self.startup_state(gain=.75, latency=0, scroll=91)
        restored = self.session(variant=117, startup_state=state)
        magic, ready = restored.message()
        self.assertEqual((magic, ready[80:82]), (b"HXVIRD04", b"\x01\x01"))
        self.assertEqual(struct.unpack_from("<If", ready[84:], 100), (0, .75))
        self.assertEqual(ready[-8:], b"HXCT" + struct.pack("<I", 91))
        _, inputs, (magic, pcm) = restored.render(7)
        self.assertEqual(magic, b"HXVB")
        self.assertEqual(struct.unpack("<14f", pcm[64:]), tuple(v * .375 for v in inputs))
        self.terminal(restored, 0, message=restored.stop())

    def test_restore_parameter_echo_and_empty_restart_do_not_change_restored_pcm(self):
        for variant in (118, 119):
            with self.subTest(variant=variant):
                state = self.startup_state(gain=.75, latency=0, scroll=91)
                session = self.session(variant=variant, startup_state=state)
                magic, ready = session.message()
                self.assertEqual((magic, ready[80:82]), (b"HXVIRD04", b"\x01\x01"))
                self.assertEqual(struct.unpack_from("<If", ready[84:], 100), (0, .75))
                self.assertEqual(ready[-8:], b"HXCT" + struct.pack("<I", 91))
                _, inputs, (magic, pcm) = session.render(7)
                self.assertEqual(magic, b"HXVB")
                self.assertEqual(struct.unpack("<14f", pcm[64:]), tuple(v * .375 for v in inputs))
                self.terminal(session, 0, message=session.stop())

    def test_startup_state_malformed_source_or_presence_never_enters_the_module(self):
        captured = self.startup_state()
        cases = []
        for offset in [0, 4, 5, 6, 8, 24, 56, 88, 92]:
            bad = bytearray(captured); bad[offset] ^= 0x80; cases.append(bytes(bad))
        cases.extend([b"", captured[:-1], captured + bytes(4097 - len(captured)),
                      self.startup_state(component=False, controller=False)])
        for index, bad in enumerate(cases):
            with self.subTest(index=index):
                self.trace.unlink(missing_ok=True)
                session = self.session(variant=37, auto_present=False, startup_state=bad)
                self.error(session, 1)
                self.terminal(session, 1)
                self.assertFalse(self.trace.exists(), "all startup bytes validate before actual module entry")

    def test_restore_sdk_refusal_readonly_and_retained_streams_never_fall_back_to_defaults(self):
        for variant in [38, 42, 46, 50, 51, 52, 53, 54, 55, 56]:
            with self.subTest(variant=variant):
                session = self.session(variant=variant, startup_state=self.startup_state())
                expected = 26 if variant == 56 else 14
                self.error(session, expected)
                retained = variant == 52
                self.terminal(session, 10 if retained else expected, closed=not retained, unloaded=not retained)
                self.assertEqual((self.observation()[8], self.observation()[10]), (0, 0))
                if variant == 55:
                    self.assertEqual(self.restore_observation()[:3], (0, 0, 0), "controller topology refuses before applying component state")

    def test_startup_absent_controller_and_empty_chunks_keep_distinct_actual_meanings(self):
        session = self.session(variant=55, startup_state=self.startup_state(controller=False))
        magic, ready = session.message()
        self.assertEqual(magic, b"HXVIRD04")
        self.assertEqual(ready[80], 1)
        self.assertEqual(struct.unpack_from("<II", ready, 172), (32, 0))
        self.terminal(session, 0, message=session.stop())
        for component_empty in [True, False]:
            with self.subTest(component_empty=component_empty):
                state = bytearray(self.startup_state())
                if component_empty:
                    state = state[:96];state[5] = 1;state[88:96] = bytes(8)
                else:
                    state = state[:128];state[92:96] = bytes(4)
                session = self.session(variant=37, startup_state=bytes(state))
                self.error(session, 14)
                self.terminal(session, 14)
                expected = (1, 0, 0) if component_empty else (1, 1, 1)
                self.assertEqual(self.restore_observation()[:3], expected)

    def test_restore_foreign_exception_and_crash_never_claim_cleanup_or_processing(self):
        for variant in [39, 40, 43, 44, 47, 48]:
            with self.subTest(variant=variant):
                session = self.session(variant=variant, startup_state=self.startup_state())
                if variant in [39, 43, 47]:
                    self.error(session, 12)
                    self.assertEqual(session.process.wait(timeout=10), 12)
                else:
                    with self.assertRaises(AssertionError): session.message()
                    self.assertEqual(session.process.wait(timeout=10), -11)
                observed = self.observation()
                self.assertEqual((observed[4], observed[8], observed[10]), (0, 0, 0))

    def test_facts_precede_all_presentation_capture_and_processing_then_acknowledge_exact_values(self):
        session = self.session(auto_present=False)
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIFA03")
        self.assertEqual(body[:126], session.facts_prefix)
        self.assertEqual(struct.unpack_from("<IIB", body, 126), (0, 0, 1))
        observed = self.observation()
        self.assertEqual(observed[0], 14)  # queried before any setter call
        self.assertEqual(observed[2:4], (1, 1))
        self.assertEqual((observed[8], observed[10]), (0, 0))
        session.presentation((77, 91))
        self.ready(session)
        session.render(8)
        self.terminal(session, 0, message=session.stop())

    def test_source_tail_report_crosses_facts_and_ready_without_changing_pcm_extent(self):
        for tail in (0, 3, 0xffffffff):
            with self.subTest(tail=tail):
                session = self.session(auto_present=False, tail_frames=tail)
                magic, facts = session.message()
                self.assertEqual(magic, b"HXVIFA03")
                self.assertEqual(struct.unpack_from("<IIB", facts, 126), (0, tail, 1))
                self.assertEqual(facts[135:], session.restore_suffix)
                self.assertEqual(self.observation()[10], 0)
                session.presentation((77, 91))
                self.ready(session)
                header, inputs, (magic, output) = session.render(8)
                self.assertEqual((magic, output[:64]), (b"HXVB", header))
                self.assertEqual(struct.unpack("<16f", output[64:]), tuple(v*.5 for v in inputs))
                terminal = self.terminal(session, 0, message=session.stop())
                self.assertEqual(terminal[5:7], (1, 8))

    def test_unavailable_stale_and_wrong_phase_presentation_never_start_dsp(self):
        for case in ("unknown", "stale", "pcm"):
            with self.subTest(case=case):
                session = self.session(auto_present=False)
                self.assertEqual(session.message()[0], b"HXVIFA03")
                if case == "unknown":
                    session.presentation(None)
                elif case == "stale":
                    session.presentation((77, 91), binding=(1, 99, 3, 4))
                else:
                    session.socket.sendall(session.packet(8))
                self.error(session, 5 if case == "unknown" else 8)
                self.terminal(session, 5 if case == "unknown" else 8)
                self.assertEqual((self.observation()[8], self.observation()[10]), (0, 0))

    def test_presentation_changed_latency_invalidates_the_plan_before_processing(self):
        session = self.session(variant=36, auto_present=False)
        self.assertEqual(session.message()[0], b"HXVIFA03")
        session.presentation((77, 91))
        self.error(session, 5)
        self.terminal(session, 5)
        self.assertEqual((self.observation()[8], self.observation()[10]), (0, 0))

    def test_explicit_unknown_presentation_starts_and_renews_the_same_effect(self):
        session = self.session(auto_present=False)
        self.assertEqual(session.message()[0], b"HXVIFA03")
        session.presentation(None, allow_unknown=True)
        magic, ready = session.message()
        self.assertEqual((magic, ready[70:72]), (b"HXVIRD04", b"\x02\0"))
        self.assertEqual(ready[-8:], bytes(8))  # SDK captured both unknown sentinels
        _, _, (magic, _) = session.render(7)
        self.assertEqual(magic, b"HXVB")
        session.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVQ", 1, 0, 0,
                                           *BINDING, session.sequence, session.frames))
        self.assertEqual(session.message()[0], b"HXVIQF02")
        session.socket.sendall(struct.pack("<4sHHHB5x6Q", b"HXVR", 1, 0, 0, 2,
                                           *BINDING, 3, 5))
        magic, resumed = session.message()
        self.assertEqual((magic, resumed[-2:]), (b"HXVIRR02", b"\x02\0"))
        binding = (*BINDING[:3], 5)
        session.socket.sendall(session.packet(1, binding=binding) + struct.pack("<ff", .25, -.5))
        magic, pcm = session.message()
        self.assertEqual(magic, b"HXVB")
        self.assertEqual(struct.unpack("<ff", pcm[64:]), (.15625, -.3125))
        session.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVE", 1, 0, 0, *binding, 2, 8))
        magic, terminal = session.message()
        self.assertEqual((magic, struct.unpack("<H6QIBB", terminal)),
                         (b"HXVITR01", (1, *binding, 2, 8, 0, 1, 1)))
        self.assertEqual(session.process.wait(timeout=10), 0)

    def test_startup_latency_notice_with_unchanged_report_allows_processing(self):
        session = self.session(variant=110)
        self.ready(session, controller=True)
        _, _, (magic, _) = session.render(7)
        self.assertEqual(magic, b"HXVB")
        self.terminal(session, 0, message=session.stop())

    def test_startup_latency_notice_is_reactivated_before_accepting_the_report(self):
        session = self.session(variant=111, auto_present=False)
        self.assertEqual(session.message()[0], b"HXVIFA03")
        session.presentation(None, allow_unknown=True)
        self.error(session, 5)
        self.terminal(session, 5)
        self.assertEqual((self.observation()[8], self.observation()[10]), (0, 0))

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

    def test_effect_disables_trailing_auxiliary_input_and_renders_only_main(self):
        session = self.session(variant=106, channels=(1, 2))
        self.ready(session, controller=True)
        header, inputs, (magic, reply) = session.render(7)
        self.assertEqual((magic, reply[:64]), (b"HXVB", header))
        self.assertEqual(struct.unpack("<14f", reply[64:]),
                         tuple(value * .5 for value in inputs[::2] for _ in range(2)))
        self.terminal(session, 0, message=session.stop())

    def test_optional_presentation_and_missing_state_are_not_fabricated(self):
        shutil.rmtree(self.bundle)
        source = self.fixtures.parent / BOOT.APP / "Contents/Resources/fixtures/effect.vst3"
        shutil.copytree(source, self.bundle)
        cid = bytes.fromhex("48455833010203040506070809101100")
        session = self.session(cid=cid, maximum=64)
        magic, body = session.message()
        self.assertEqual(magic, b"HXVIRD04", body)
        self.assertEqual(body[70:72], bytes([0, 1]))  # optional absent; missing != empty
        self.assertEqual(body[84:92], b"HXVS\x01\x00\x00\x00")
        self.assertEqual(struct.unpack_from("<II", body, 172), (0, 0))
        self.assertEqual(len(body), 180)
        header, inputs, (magic, output) = session.render(7)
        self.assertEqual((magic, output[:64]), (b"HXVB", header))
        self.assertEqual(struct.unpack("<14f", output[64:]), tuple(v*.5 for v in inputs))
        self.terminal(session, 0, message=session.stop())

    def test_null_false_optional_query_keeps_separate_controller_and_pcm(self):
        session = self.session(variant=112, auto_present=False)
        self.assertEqual(session.message()[0], b"HXVIFA03")
        session.presentation((77, 91))
        magic, ready = session.message()
        self.assertEqual((magic, ready[70:72]), (b"HXVIRD04", b"\0\x01"))
        self.assertEqual(ready[-8:], b"HXCT" + struct.pack("<I", 7))
        _, inputs, (magic, pcm) = session.render(7)
        self.assertEqual(magic, b"HXVB")
        self.assertEqual(struct.unpack("<14f", pcm[64:]), tuple(v * .5 for v in inputs))
        self.terminal(session, 0, message=session.stop())

    def test_null_success_or_missing_required_processor_refuses_instance_open(self):
        for variant in (113, 114):
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.error(session, 13)
                self.terminal(session, 13)

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

    def test_setup_refusal_reports_the_sdk_stage(self):
        for variant, code in [(1, 21), (3, 23), (7, 24)]:
            with self.subTest(variant=variant):
                session = self.session(variant=variant)
                self.error(session, code)
                self.terminal(session, code)

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
                self.assertEqual(self.observation()[0], 15 if variant == 25 else 7)
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

    def test_current_capture_requires_quiescence_exact_prefix_and_new_operation(self):
        base = struct.pack("<4sHHH6x7Q", b"HXVC", 1, 0, 0, *BINDING, 0, 0, 1)
        for mode in ("live", "ordinal", "duplicate", "prefix", "binding", "zero"):
            with self.subTest(mode=mode):
                session = self.session()
                self.ready(session)
                if mode != "live":
                    session.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVQ", 1, 0, 0, *BINDING, 0, 0))
                    self.assertEqual(session.message()[0], b"HXVIQF02")
                packet = bytearray(base)
                if mode == "duplicate":
                    session.socket.sendall(packet)
                    magic, body = session.message()
                    self.assertEqual(magic, b"HXVICS01")
                    self.assertEqual(struct.unpack_from("<H7Q", body), (1, *BINDING, 0, 0, 1))
                elif mode == "ordinal":
                    struct.pack_into("<Q", packet, 64, 2)
                elif mode == "zero":
                    struct.pack_into("<Q", packet, 64, 0)
                elif mode == "prefix":
                    struct.pack_into("<Q", packet, 48, 1)
                elif mode == "binding":
                    struct.pack_into("<Q", packet, 40, BINDING[3] + 1)
                session.socket.sendall(packet)
                self.error(session, 8)
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)

    def test_live_capture_poll_keeps_pcm_prefix_and_rejects_consumed_operation(self):
        session = self.session()
        self.ready(session)

        def capture(magic, operation):
            session.socket.sendall(struct.pack("<4sHHH6x7Q", magic, 1, 0, 0,
                                              *BINDING, session.sequence, session.frames, operation))
            return session.message()

        magic, body = capture(b"HXVL", 1)
        self.assertEqual(magic, b"HXVLCA01")
        self.assertEqual(body, struct.pack("<H7QBH", 1, *BINDING, 0, 0, 1, 0, 0))
        header, _, (magic, pcm) = session.render(8)
        self.assertEqual(magic, b"HXVB")
        self.assertEqual(pcm[:64], header)
        for _ in range(10000):
            magic, body = capture(b"HXVP", 1)
            self.assertEqual(magic, b"HXVLCA01")
            self.assertEqual(body[:58], struct.pack("<H7Q", 1, *BINDING, 1, 8, 1))
            if body[58]:
                break
        else:
            self.fail("actual SDK live capture did not complete")
        self.assertEqual(body[58], 1)
        self.assertEqual(struct.unpack_from("<H", body, 59)[0], len(body) - 61)
        self.assertEqual(body[61:65], b"HXVS")
        self.assertEqual(body[69:85], CID)
        self.assertEqual(capture(b"HXVP", 1), (b"HXVIER01", struct.pack("<HB4QI", 1, 1, *BINDING, 8)))
        self.assertEqual(session.process.wait(timeout=10), 8)

    def test_live_capture_rejects_stale_or_overlapping_control(self):
        for bad in ("zero", "skipped", "prefix", "binding", "overlap", "poll_without_start"):
            with self.subTest(bad=bad):
                session = self.session()
                self.ready(session)

                def packet(magic, operation, binding=BINDING, sequence=0):
                    return struct.pack("<4sHHH6x7Q", magic, 1, 0, 0, *binding, sequence, 0, operation)

                if bad == "overlap":
                    session.socket.sendall(packet(b"HXVL", 1))
                    self.assertEqual(session.message()[0], b"HXVLCA01")
                request = packet(b"HXVP" if bad == "poll_without_start" else b"HXVL",
                                 0 if bad == "zero" else 2 if bad in ("skipped", "overlap") else 1,
                                 (1, 2, 3, 99) if bad == "binding" else BINDING,
                                 1 if bad == "prefix" else 0)
                session.socket.sendall(request)
                self.assertEqual(session.message(), (b"HXVIER01", struct.pack("<HB4QI", 1, 1, *BINDING, 8)))
                self.assertEqual(session.process.wait(timeout=10), 8)

    def test_epoch_control_rejects_bad_prefix_and_resume_before_quiescence(self):
        base = bytearray(struct.pack("<4sHHH6x6Q", b"HXVQ", 1, 0, 0, *BINDING, 0, 0))
        cases = []
        for offset in (4, 6, 8, 10, 16, 24, 32, 40, 48, 56):
            bad = base.copy()
            bad[offset] ^= 1
            cases.append(bytes(bad))
        cases.append(struct.pack("<4sHHHB5x6QII", b"HXVR", 1, 0, 0, 1,
                                 *BINDING, 3, 5, 77, 91))
        for packet in cases:
            with self.subTest(packet=packet.hex()):
                session = self.session()
                self.ready(session)
                session.socket.sendall(packet)
                self.error(session, 8)
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)
                self.assertEqual(self.observation()[8], 1)

    def test_quiescent_raw_resume_refuses_stale_binding_flags_and_shape_before_restart(self):
        base = bytearray(struct.pack("<4sHHHB5x6QII", b"HXVR", 1, 0, 0, 1,
                                    *BINDING, 3, 5, 77, 91))
        cases = []
        for offset in (4, 6, 8, 11, 16, 24, 32, 40):
            bad = base.copy()
            bad[offset] ^= 1
            cases.append(bytes(bad))
        for graph, epoch in ((2, 5), (3, 4), (3, 0)):
            bad = base.copy()
            struct.pack_into("<QQ", bad, 48, graph, epoch)
            cases.append(bytes(bad))
        bad = base.copy()
        bad[10] = 3
        cases.append(bytes(bad))
        for packet in cases:
            with self.subTest(packet=packet.hex()):
                session = self.session()
                self.ready(session)
                session.socket.sendall(struct.pack("<4sHHH6x6Q", b"HXVQ", 1, 0, 0,
                                                   *BINDING, 0, 0))
                magic, body = session.message()
                self.assertEqual(magic, b"HXVIQF02")
                self.assertEqual(struct.unpack_from("<QQ", body, len(body)-16), (0, 0))
                session.socket.sendall(packet)
                self.error(session, 8)
                self.terminal(session, 8)
                self.assertEqual(self.observation()[10], 0)
                self.assertEqual(self.observation()[8:10], (1, 1))

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
                      b"HXVICF03" + struct.pack("<I", 8449),
                      b"HXVICF03" + struct.pack("<I", 5) + b"short",
                      b"HXVICF03" + struct.pack("<I", 50) + b"short"]:
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
        self.assertEqual(session.message()[0], b"HXVIRD04")
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
        with self.assertRaises(subprocess.CalledProcessError):
            delivery.verify(self.app)
        binary.write_bytes(original)
        receipt = self.app / delivery.INSTANCE_RECEIPT
        data = json.loads(receipt.read_text())
        data["source_commit"] = "0" * 40
        receipt.write_text(json.dumps(data))
        # Re-seal this owned negative fixture so the test reaches source
        # validation rather than stopping at the intentionally broken seal.
        self.manifest["vst3_instance"]["receipt_sha256"] = delivery.sha(receipt)
        self.manifest["files"] = delivery.inventory(self.app, signed_payload=True)
        delivery.write_json(self.app / "Contents/Resources/test-build.json", self.manifest)
        delivery.sign_bundle(self.app, self.signing)
        with self.assertRaisesRegex(delivery.DeliveryError, "exact clean candidate"):
            delivery.verify(self.app)
