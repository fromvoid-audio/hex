"""Identity refusal tests; native evidence requires explicit original receipts."""
import importlib.util
import json
import os
import plistlib
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

NATIVE = Path(__file__).resolve().parents[2] / "crates/hex-plugin-scan/native"
sys.path.insert(0, str(NATIVE))
try:
    SPEC = importlib.util.spec_from_file_location("hex_macos_bootstrap", NATIVE / "macos_bootstrap.py")
    BOOT = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(BOOT)
finally:
    sys.path.pop(0)


@unittest.skipUnless(os.environ.get("HEX_VST3_INSTANCE_RECEIPT"), "explicit instance helper receipt required")
class InstanceDisplayTests(unittest.TestCase):
    def test_source_bound_editor_helper_declares_high_resolution(self):
        receipt = Path(os.environ["HEX_VST3_INSTANCE_RECEIPT"]).resolve()
        BOOT.verify(receipt)
        info = plistlib.loads((receipt.parent / BOOT.INSTANCE_APP / "Contents/Info.plist").read_bytes())
        self.assertEqual(info["CFBundleIdentifier"], "org.portalsurfer.hex.vst3.instance")
        self.assertIs(info.get("LSUIElement"), True)
        self.assertIs(info.get("NSHighResolutionCapable"), True)

    def test_resealed_manifest_cannot_mask_missing_high_resolution_capability(self):
        original = Path(os.environ["HEX_VST3_INSTANCE_RECEIPT"]).resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(original.parent / BOOT.INSTANCE_APP, root / BOOT.INSTANCE_APP)
            receipt = root / "controlled-macos.json"
            data = json.loads(original.read_text())
            info_path = root / BOOT.INSTANCE_APP / "Contents/Info.plist"
            info = plistlib.loads(info_path.read_bytes())
            del info["NSHighResolutionCapable"]
            info_path.write_bytes(plistlib.dumps(info))
            data["bundle_files"] = BOOT.inventory(root / BOOT.INSTANCE_APP)
            receipt.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "lacks high-resolution support"):
                BOOT.verify(receipt)


@unittest.skipUnless(
    sys.platform == "darwin" and os.environ.get("HEX_VST3_MACOS_SDK"),
    "explicit macOS VST3 SDK required for the hidden AppKit fixture",
)
class EditorFrameTests(unittest.TestCase):
    def test_controller_feedback_respects_newer_native_edits(self):
        sdk = Path(os.environ["HEX_VST3_MACOS_SDK"])
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "host-feedback"
            subprocess.run(
                ["xcrun", "clang++", "-std=c++17", "-arch", "arm64", "-O2",
                 "-Wno-deprecated-declarations", "-I" + str(sdk),
                 str(NATIVE / "macos_host_feedback_fixture.mm"), "-framework", "AppKit",
                 "-o", str(binary)],
                check=True, timeout=120,
            )
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_editor_frame_negotiates_plugin_and_window_resizes(self):
        sdk = Path(os.environ["HEX_VST3_MACOS_SDK"])
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "editor-resize"
            subprocess.run(
                ["xcrun", "clang++", "-std=c++17", "-arch", "arm64", "-I" + str(sdk),
                 str(NATIVE / "macos_editor_resize_fixture.mm"), "-framework", "AppKit",
                 "-o", str(binary)],
                check=True, timeout=120,
            )
            result = subprocess.run([str(binary)], capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)


class MachHeaderTests(unittest.TestCase):
    def test_wrong_role_and_architecture_refuse(self):
        # Parser component evidence only; not an executable native fixture.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "header"
            for cpu, kind in [(0x01000007, 2), (0x0100000C, 8)]:
                path.write_bytes(struct.pack("<8I", 0xFEEDFACF, cpu, 0, kind, 1, 8, 0, 0) + b"\0" * 8)
                with self.subTest(cpu=cpu, kind=kind), self.assertRaisesRegex(ValueError, "native arm64"):
                    BOOT.macho(path, 2)

    def test_truncation_and_command_bounds_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "header"
            for data in [b"\xcf\xfa", struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 8, 0, 0)]:
                path.write_bytes(data)
                with self.subTest(size=len(data)), self.assertRaises(ValueError):
                    BOOT.macho(path, 2)


@unittest.skipUnless(os.environ.get("HEX_VST3_MACOS_RECEIPT"), "explicit original macOS fixture receipt required")
class NativeBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.original = Path(os.environ["HEX_VST3_MACOS_RECEIPT"]).resolve()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(self.original.parent / BOOT.APP, self.root / BOOT.APP)
        self.receipt = self.root / "controlled-macos.json"
        shutil.copyfile(self.original, self.receipt)

    def test_actual_native_identity_and_effect_instrument_pcm(self):
        self.assertEqual(BOOT.verify(self.receipt, run=True), {
            "version": 1, "scope": "original-macos-bootstrap", "effect_frames": 64, "instrument_frames": 64})

    def processing(self, scenario):
        run = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--processing-self-test", str(scenario)],
                             capture_output=True, text=True, timeout=15)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertLess(len(run.stdout), 4096)
        value = json.loads(run.stdout)
        self.assertEqual(value["scope"], "original-macos-processing")
        self.assertEqual(value["scenario"], scenario)
        for key in ("pcm", "descriptors", "active_preserved", "closed", "unloaded"):
            self.assertIs(value[key], True, (scenario, key, value))
        observed = value["observation"]
        self.assertEqual(len(observed), 13)
        self.assertEqual(observed[0], 0)  # actual SDK fixture thread/lifetime violations
        self.assertEqual(observed[1:4], [observed[1]] * 3)
        self.assertEqual(observed[11:], [0, 1])  # no component left; one loader factory reference
        return value, observed

    def test_processing_pcm_bus_shapes_aliasing_silence_and_rates(self):
        for scenario in list(range(10)) + [23, 24, 25, 27, 28]:
            with self.subTest(scenario=scenario):
                value, observed = self.processing(scenario)
                self.assertEqual(value["configuration"], 0)
                self.assertEqual(observed[7:11], [1, 1, 3, 0])

    def test_stopped_flush_and_local_descriptor_refusals_do_not_replay_dsp(self):
        for scenario in (10, 11, 12, 29):
            with self.subTest(scenario=scenario):
                value, observed = self.processing(scenario)
                self.assertEqual(value["configuration"], 0)
                self.assertEqual(observed[9], 4 if scenario == 10 else 3)
                self.assertEqual(observed[10], 2 if scenario == 10 else 0)
                self.assertEqual(observed[4], 1)  # Float64/local validation never calls arrangements

    def test_failed_candidate_configuration_preserves_running_instance(self):
        for scenario in (13, 14):
            with self.subTest(scenario=scenario):
                _value, observed = self.processing(scenario)
                self.assertEqual(observed[1:5], [2, 2, 2, 2])
                self.assertEqual(observed[7:11], [1, 1, 3, 0])
                self.assertEqual(observed[5], 1 if scenario == 13 else 2)

    def test_configuration_refusals_and_foreign_output_fault_retirement(self):
        for scenario, refusal in ((15, 6), (16, 4), (17, 3), (18, 8), (31, 6)):
            with self.subTest(scenario=scenario):
                value, observed = self.processing(scenario)
                self.assertEqual(value["configuration"], refusal)
                self.assertEqual(observed[7:11], [0, 0, 0, 0])
        value, observed = self.processing(30)
        self.assertEqual(value["flush"], 4)
        self.assertEqual(observed[7:11], [0, 0, 0, 1])
        for scenario in (19, 20, 21, 22, 26):
            with self.subTest(scenario=scenario):
                _value, observed = self.processing(scenario)
                self.assertEqual(observed[7:9], [1, 1])
                self.assertEqual(observed[9], 1)  # one attempted DSP call, even when the plug-in refuses

    @staticmethod
    def stream_packet(sequence, *, graph=3, frames=8, events=True):
        header = struct.pack("<4sHHH6x6Q", b"HXVB", 1, frames, int(events),
                             1, 2, graph, 4, sequence, sequence * frames)
        pcm = struct.pack("<" + "ff" * frames, *([0.75, -0.5] * frames))
        event = struct.pack("<HB3B2x", frames - 1, 3, 0x90, 60, 100) if events else b""
        return header + pcm + event

    def stream(self, data):
        BOOT.verify(self.receipt)
        return subprocess.run([str(self.root / BOOT.EXECUTABLE), "--stream-fixture"],
                              input=data, capture_output=True, timeout=10)

    def test_persistent_stream_uses_one_real_processor_and_copied_event_metadata(self):
        packets = [self.stream_packet(n) for n in range(32)]
        run = self.stream(b"".join(packets))
        self.assertEqual(run.returncode, 0, run.stderr.decode())
        self.assertEqual(len(run.stdout), sum(map(len, packets)))
        self.assertIn(b"blocks=32 closed=1 violations=0", run.stderr)
        size = len(packets[0])
        for sequence, request in enumerate(packets):
            reply = run.stdout[sequence * size:(sequence + 1) * size]
            self.assertEqual(reply[:64], request[:64])
            self.assertEqual(reply[-8:], request[-8:], "MIDI transport metadata, not SDK event processing")
            scale = 0.5 if sequence == 0 else 0.25  # original stateful fixture's literal oracle
            self.assertEqual(struct.unpack("<16f", reply[64:-8]), tuple([0.75 * scale, -0.5 * scale] * 8))

    def test_stream_refuses_bad_bounds_truncation_and_stale_identity_without_dsp_replay(self):
        first = self.stream_packet(0)
        bad_count = bytearray(first)
        bad_count[8:10] = struct.pack("<H", 65)
        nonfinite = bytearray(first)
        nonfinite[64:68] = struct.pack("<f", float("nan"))
        cases = [bytes(bad_count), bytes(nonfinite), first[:-1], self.stream_packet(0, frames=65),
                 self.stream_packet(1), first + self.stream_packet(0),
                 first + self.stream_packet(2), first + self.stream_packet(1, graph=99)]
        for data in cases:
            with self.subTest(size=len(data), tail=data[-8:]):
                run = self.stream(data)
                self.assertNotEqual(run.returncode, 0, run.stderr.decode())
                self.assertLessEqual(len(run.stdout), len(first))
                if len(data) == len(first) * 2:
                    self.assertEqual(run.stdout[:64], first[:64])
                    self.assertIn(b"blocks=1 closed=1 violations=0", run.stderr)

    def lifecycle(self, mode, failure, expected_exit=0):
        # Own original helper only; source/inventory admission before execution.
        BOOT.verify(self.receipt)
        run = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--lifecycle-self-test",
                              str(mode), str(failure)], capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, expected_exit, run.stdout + run.stderr)
        self.assertLessEqual(len(run.stdout.encode()), 4096)
        value = json.loads(run.stdout)
        self.assertEqual(value["version"], 1)
        self.assertEqual(value["scope"], "original-macos-instance-lifecycle")
        self.assertEqual((value["mode"], value["failure"]), (mode, failure))
        observation = value["observation"]
        self.assertLessEqual(len(observation), 135)
        self.assertGreaterEqual(len(observation), 7)
        self.assertEqual(observation[6], len(observation) - 7)
        self.assertEqual(observation[0], 0, value)  # independent SDK order/thread/ref oracle
        self.assertTrue(value["pcm"], value)  # two literal, different block outputs
        self.assertTrue(value["stable_close"], value)  # no repeated foreign teardown
        return value, observation[7:]

    def assert_persistent_lifecycle(self, mode):
        with self.subTest(mode=mode, failure=0):
            normal, trace = self.lifecycle(mode, 0)
            for field in ("opened", "configured", "started", "processed", "stopped", "closed", "unloaded"):
                self.assertTrue(normal[field], normal)
            observation = normal["observation"]
            self.assertEqual(observation[1:6], [1, int(mode == 1), 2, 0, 1])
            # One actual component persists across both process calls. Combined
            # interfaces get no duplicate initialization/termination or self-connect.
            self.assertEqual(trace.count(2), 1)
            self.assertEqual(trace.count(24), 1)
            self.assertEqual(trace.count(5), int(mode == 1))
            self.assertEqual(trace.count(23), int(mode == 1))
            self.assertEqual(trace.count(7), int(mode == 1))
            self.assertEqual(trace.count(8), int(mode == 1))
            self.assertEqual(trace.count(15), 2)
            for before, after in ((2, 13), (13, 14), (14, 15), (15, 16), (16, 17), (17, 24)):
                self.assertLess(trace.index(before), trace.index(after))
            if mode == 1:
                self.assertLess(trace.index(7), trace.index(13))
                self.assertLess(trace.index(20), trace.index(23))
                self.assertLess(trace.index(21), trace.index(24))
        # Only real transitions for this variant, not duplicate no-op cases.
        for failure in sorted(set(trace)):
            with self.subTest(mode=mode, failure=failure):
                value, failed_trace = self.lifecycle(mode, failure)
                self.assertIn(failure, failed_trace)
                if failure >= 16:
                    # A refused teardown leaves resources in the isolated helper.
                    # No later calls, repeat cleanup, live release or module unload.
                    self.assertFalse(value["closed"], value)
                    self.assertFalse(value["unloaded"], value)
                    self.assertGreater(value["observation"][4], 0, value)
                    self.assertEqual(failed_trace[-1], failure)
                else:
                    self.assertTrue(value["closed"], value)
                    self.assertTrue(value["unloaded"], value)
                    self.assertEqual(value["observation"][4], 0, value)
                if failure <= 8:
                    self.assertFalse(value["opened"], value)
                elif failure <= 13:
                    self.assertEqual(failed_trace.count(failure), 1, "partial preparation is not replayed")
                    self.assertFalse(value["configured"], value)
                elif failure == 14:
                    self.assertEqual(failed_trace.count(14), 1, "failed processing start is not replayed")
                    self.assertFalse(value["started"], value)
                elif failure == 15:
                    self.assertEqual(failed_trace.count(15), 1, "failed foreign DSP is never replayed")
                    self.assertFalse(value["processed"], value)
                    self.assertTrue(value["stopped"], value)

    def test_persistent_component_without_controller_lifecycle(self):
        self.assert_persistent_lifecycle(0)

    def test_persistent_component_separate_controller_lifecycle(self):
        self.assert_persistent_lifecycle(1)

    def test_persistent_component_combined_controller_lifecycle(self):
        self.assert_persistent_lifecycle(2)

    def test_factory_alias_never_initializes_the_component_twice(self):
        value, trace = self.lifecycle(1, 25)
        self.assertFalse(value["opened"], value)
        self.assertTrue(value["closed"], value)
        self.assertTrue(value["unloaded"], value)
        self.assertEqual(value["observation"][1:6], [1, 0, 0, 0, 1])
        self.assertEqual(trace.count(2), 1)
        self.assertNotIn(5, trace)
        self.assertEqual(trace.count(24), 1)

    def test_refused_module_exit_is_not_followed_by_live_unload(self):
        value, _trace = self.lifecycle(1, 26, expected_exit=1)
        self.assertTrue(value["closed"], value)  # component/controller retired
        self.assertFalse(value["unloaded"], value)  # module remains owned until child exit
        self.assertEqual(value["observation"][4], 0)

    def test_lifecycle_arguments_refuse_unsupported_inputs(self):
        for mode, failure in (("3", "0"), ("0", "27"), ("-1", "0"), ("0", "999999999999"), ("a", "0")):
            with self.subTest(mode=mode, failure=failure):
                run = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--lifecycle-self-test", mode, failure],
                                     capture_output=True, timeout=10)
                self.assertEqual(run.returncode, 2)
                self.assertEqual(run.stdout, b"")

    def host_control(self, scenario):
        BOOT.verify(self.receipt)
        run = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--host-self-test", str(scenario)],
                             capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertLessEqual(len(run.stdout.encode()), 4096)
        value = json.loads(run.stdout)
        self.assertEqual((value["version"], value["scope"], value["scenario"]),
                         (1, "original-macos-host-control", scenario))
        self.assertEqual(value["violations"], 0, value)
        self.assertTrue(value["pcm"], value)  # same instance, same worker, two different literal blocks
        self.assertEqual(value["before_revision"], 1)
        self.assertEqual(value["before_value"], 0.25)
        return value

    def test_actual_sdk_sealed_editor_keeps_accepted_fifo_and_refuses_late_input(self):
        value = self.host_control(15)
        self.assertEqual((value["edit_count"], value["edit_order"], value["edit_backpressure"], value["enqueued"]), (16, 1, 1, 16))
        self.assertEqual((value["known"], value["unknown"], value["message"], value["refusals"]), (1, 1, 16, 0))
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_actual_sdk_edit_fifo_preserves_boundaries_values_and_backpressure(self):
        value=self.host_control(13)
        self.assertEqual((value["edit_count"],value["edit_order"],value["edit_backpressure"],value["enqueued"]),(16,1,1,16))
        self.assertEqual(value["refusals"],0)
        self.assertEqual(value["before_revision"],value["after_revision"],"queue admission is not a controller refresh or DSP ACK")
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_real_host_messages_and_editor_metadata_refresh(self):
        value = self.host_control(0)
        self.assertEqual(value["message"], 0, value)
        self.assertEqual(value["received"], 1)
        self.assertEqual(value["disposition"], 1)
        self.assertEqual(value["after_revision"], 2)
        self.assertEqual(value["after_value"], 0.75)
        self.assertTrue(value["refusals"] & 512, "message pool exhaustion was explicit")
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_background_host_message_during_processing_keeps_instance_healthy(self):
        value = self.host_control(14)
        self.assertEqual(value["message"], 0, value)
        self.assertEqual(value["refusals"], 0)
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_unknown_and_graph_restart_classes_refuse_without_partial_cache_update(self):
        value = self.host_control(1)
        for field in ("unknown", "graph", "known"):
            self.assertNotEqual(value[field], 0, value)
        self.assertEqual(value["dispatched"], 0)
        self.assertEqual((value["after_revision"], value["after_value"]), (1, 0.25))
        self.assertEqual(value["refusals"] & (2 | 4 | 128), 2 | 4 | 128)
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_restart_queue_has_bounded_ordered_backpressure(self):
        value = self.host_control(2)
        self.assertEqual((value["enqueued"], value["dispatched"]), (8, 8))
        self.assertNotEqual(value["callback"], 0)
        self.assertEqual((value["first_sequence"], value["last_sequence"]), (2, 9))
        self.assertEqual((value["after_revision"], value["after_value"]), (9, 0.75))
        self.assertTrue(value["refusals"] & 8)
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_malformed_recursive_and_over_capacity_metadata_preserves_the_last_snapshot(self):
        for scenario in range(3, 10):
            with self.subTest(scenario=scenario):
                value = self.host_control(scenario)
                self.assertEqual(value["callback"], 0, "callback acknowledges queued work, not application")
                self.assertEqual(value["dispatched"], 1)
                disposition, refusal = (4, 16) if scenario == 8 else (7, 1024) if scenario == 7 else (2, 32)
                self.assertEqual(value["disposition"], disposition)
                self.assertEqual((value["after_revision"], value["after_value"]), (1, 0.25))
                self.assertTrue(value["refusals"] & refusal)
                self.assertTrue(value["closed"] and value["unloaded"])

    def test_processing_thread_callbacks_refuse_before_controller_or_message_work(self):
        value = self.host_control(10)
        self.assertNotEqual(value["processing_callback"], 0)
        self.assertNotEqual(value["processing_message"], 0)
        self.assertEqual(value["worker_dispatch"], 6)
        self.assertEqual(value["dispatched"], 0)
        self.assertEqual((value["after_revision"], value["after_value"]), (1, 0.25))
        self.assertTrue(value["refusals"] & 1)
        self.assertTrue(value["closed"] and value["unloaded"])

    def test_retained_message_keeps_host_pool_and_module_until_process_retirement(self):
        value = self.host_control(11)
        self.assertEqual(value["message"], 0)
        self.assertFalse(value["closed"])
        self.assertFalse(value["unloaded"])

    def test_absent_helper_refuses_before_execution(self):
        (self.root / BOOT.EXECUTABLE).unlink()
        with self.assertRaisesRegex(ValueError, "missing or changed"):
            BOOT.verify(self.receipt, run=True)

    def test_changed_fixture_and_helper_refuse_before_execution(self):
        for relative in [BOOT.EXECUTABLE, BOOT.APP / "Contents/Resources/fixtures/effect.vst3/Contents/MacOS/fixture"]:
            path = self.root / relative
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.subTest(path=str(relative)), self.assertRaisesRegex(ValueError, "missing or changed"):
                BOOT.verify(self.receipt, run=True)
            path.write_bytes(original)

    def test_actual_x86_64_binary_refuses_even_with_updated_manifest(self):
        # A real wrong-architecture Mach-O, not a patched header or hash-only refusal.
        binary = self.root / BOOT.EXECUTABLE
        compiler = BOOT.command(["xcrun", "--find", "clang++"])
        subprocess.run([compiler, "-arch", "x86_64", "-x", "c++", "-", "-o", str(binary)],
                       input="int main() { return 0; }\n", text=True, check=True, timeout=120)
        data = json.loads(self.receipt.read_text())
        data["bundle_files"] = BOOT.inventory(self.root / BOOT.APP)
        self.receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "native arm64"):
            BOOT.verify(self.receipt, run=True)

    def test_helper_itself_refuses_changed_fixture_before_loading(self):
        path = self.root / BOOT.APP / "Contents/Resources/fixtures/effect.vst3/Contents/MacOS/fixture"
        path.write_bytes(path.read_bytes() + b"changed")
        # Bypass the Python verifier deliberately: native compiled-in identity
        # must still refuse the changed original module, before CFBundle load.
        result = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--self-test"],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertIn("original fixture smoke failed", result.stderr)

    def test_redirected_bundle_executable_never_runs_even_after_resealing(self):
        sdk = Path(os.environ.get("HEX_VST3_MACOS_SDK", BOOT.ROOT / "artifacts/vst3-sdk"))
        if not sdk.is_dir():
            self.skipTest("pinned native build SDK required for executable-selection regression")
        bundle = self.root / BOOT.APP / "Contents/Resources/fixtures/effect.vst3"
        redirected = bundle / "Contents/MacOS/redirected"
        compiler = BOOT.command(["xcrun", "--find", "clang++"])
        # Real original effect plus a load-time marker: refusal must occur
        # before foreign initialization, not only after comparing its output.
        marker_source = '''#include <cstdio>
#include <cstdlib>
__attribute__((constructor)) static void marker() {
    const char* path=std::getenv("HEX_VST3_REDIRECT_MARKER");
    if(path) { auto* f=std::fopen(path,"w"); if(f) { std::fputs("executed",f); std::fclose(f); } }
}
'''
        subprocess.run([compiler, "-std=c++17", "-arch", "arm64", "-bundle", "-I" + str(sdk),
                        str(NATIVE / "macos_fixture.cpp"), "-x", "c++", "-", "-framework", "CoreFoundation",
                        "-o", str(redirected)], input=marker_source, text=True, check=True, timeout=120)
        info_path = bundle / "Contents/Info.plist"
        info = plistlib.loads(info_path.read_bytes())
        info["CFBundleExecutable"] = "redirected"
        info_path.write_bytes(plistlib.dumps(info))
        for target in [bundle, self.root / BOOT.APP]:
            subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(target)],
                           check=True, capture_output=True, timeout=120)
        data = json.loads(self.receipt.read_text())
        data["bundle_files"] = BOOT.inventory(self.root / BOOT.APP)
        self.receipt.write_text(json.dumps(data))
        marker = self.root / "redirected-ran"
        env = {**os.environ, "HEX_VST3_REDIRECT_MARKER": str(marker)}
        # Invoke directly as well as through the manifest verifier, so each
        # boundary must refuse the actual selection despite matching hashes.
        result = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--self-test"],
                                env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(marker.exists())
        with self.assertRaisesRegex(ValueError, "fixture executable selection"):
            BOOT.verify(self.receipt, run=True)

    def test_hung_signing_is_bounded_and_publishes_no_receipt(self):
        sdk = Path(os.environ.get("HEX_VST3_MACOS_SDK", BOOT.ROOT / "artifacts/vst3-sdk"))
        if not sdk.is_dir():
            self.skipTest("pinned native build SDK required for signing-failure injection")
        run = subprocess.run

        def signing_fault(args, **kwargs):
            if args[0] == "/usr/bin/codesign":
                # Preserve whether the caller supplied a deadline, scale it for
                # a fast fault injection, and use a real owned sleeping child.
                return run([sys.executable, "-c", "import time; time.sleep(0.1)"],
                           check=True, timeout=0.01 if kwargs.get("timeout") else None)
            return run(args, **kwargs)

        with tempfile.TemporaryDirectory(dir=BOOT.ROOT / "artifacts") as directory:
            output = Path(directory) / "failed-build"
            with mock.patch.object(BOOT.subprocess, "run", side_effect=signing_fault):
                with self.assertRaises(subprocess.TimeoutExpired):
                    BOOT.build(sdk, output)
            self.assertFalse((output / "controlled-macos.json").exists())

    def test_helper_does_not_accept_an_installed_module_path(self):
        result = subprocess.run([str(self.root / BOOT.EXECUTABLE), "/Library/Audio/Plug-Ins/VST3"],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertIn("arbitrary paths refused", result.stderr)

    def test_stale_source_and_linked_member_refuse(self):
        data = json.loads(self.receipt.read_text())
        data["source_sha256"] = "0" * 64
        self.receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "stale bootstrap"):
            BOOT.verify(self.receipt, run=True)
        shutil.copyfile(self.original, self.receipt)
        (self.root / BOOT.APP / "Contents/linked").symlink_to(self.original)
        with self.assertRaisesRegex(ValueError, "linked bundle"):
            BOOT.verify(self.receipt, run=True)


if __name__ == "__main__":
    unittest.main()
