"""Finite dependency/configuration/callback sensitivity, no runtime proof."""
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from custody_mutex_policy import dependency_violations, source_violations, ROOT, ELIGIBLE, CALLBACK_FILES


def exact():
    return {'dependencies': {'spin': {'version': '=0.9.9', 'default-features': False,
                                     'features': ['mutex', 'spin_mutex', 'std']}}}


class CustodyMutexPolicyTests(unittest.TestCase):
    def test_exact_audio_dependency_only(self):
        self.assertEqual(dependency_violations('hex-audio', exact()), [])
        self.assertTrue(dependency_violations('hex-engine', exact()))

    def test_default_alias_features_and_version_changes_refuse(self):
        for key, value in [('default-features', True), ('version', '=0.9.8'),
                           ('features', ['mutex', 'spin_mutex', 'ticket_mutex', 'std'])]:
            manifest = exact()
            manifest['dependencies']['spin'][key] = value
            self.assertTrue(dependency_violations('hex-audio', manifest))
        manifest = exact()
        manifest['dependencies']['spin'].pop('default-features')
        self.assertTrue(dependency_violations('hex-audio', manifest))

    def test_dev_or_target_relocation_does_not_authorize_production(self):
        self.assertTrue(dependency_violations('hex-audio', {'dev-dependencies': exact()['dependencies']}))
        self.assertTrue(dependency_violations('hex-audio', {'target': {'cfg(foo)': exact()}}))

    def test_additional_field_or_foreign_primitive_refuses(self):
        self.assertTrue(source_violations(ROOT, 'bank: CustodyMutex<Bank>,\nextra: CustodyMutex<u32>,'))
        self.assertTrue(source_violations(ELIGIBLE, 'bank: CustodyMutex<NativeBank>,'))
        self.assertTrue(source_violations('crates/hex-engine/src/foreign.rs', 'use spin::mutex::SpinMutex;'))

    def test_callback_cannot_call_yielding_control_helper(self):
        for path in CALLBACK_FILES:
            for body in ['lock(&self.bank);', 'cell.lock_recover_on_control();', 'std::thread::yield_now();']:
                self.assertTrue(source_violations(path, body))
            self.assertEqual(source_violations(path, 'self.bank.try_lock();'), [])

    def test_try_path_cannot_grow_a_retry_or_use_feature_alias(self):
        from custody_mutex_policy import WRAPPER
        body = (Path(__file__).resolve().parents[2] / WRAPPER).read_text()
        self.assertTrue(source_violations(WRAPPER, body.replace('SpinMutex<T, spin::Yield>', 'spin::Mutex<T>')))
        self.assertTrue(source_violations(WRAPPER, body.replace('let value = self', 'loop {}\n        let value = self', 1)))

    def test_actual_repository_contract(self):
        from custody_mutex_policy import repository_violations
        self.assertEqual(repository_violations(Path(__file__).resolve().parents[2]), [])

if __name__ == '__main__':
    unittest.main()
