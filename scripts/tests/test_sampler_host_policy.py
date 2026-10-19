"""Finite source-boundary sensitivity; no compiler/admission claim."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    'sampler_host_policy', Path(__file__).resolve().parents[1] / 'sampler_host_policy.py')
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)
violations = policy.violations


class SamplerHostPolicyTests(unittest.TestCase):
    def test_actual_file_owner_is_allowed(self):
        self.assertEqual(violations(
            'crates/hex-session/src/service/main_control_file.rs',
            "impl<'a> hex_engine::sampler_host_fields::FileSlotsFields<'a> for Reserved<'a> {}"), [])

    def test_foreign_module_in_same_crate_is_refused(self):
        self.assertTrue(violations('crates/hex-session/src/random.rs',
                                   'use hex_engine::sampler_host_fields as fields;'))

    def test_foreign_crate_and_aliased_import_are_refused(self):
        for text in ['use hex_engine::sampler_host_fields::{BodyRef as X};',
                     'let body = hex_engine::sampler_host_fields::BodyRef::of(&x);',
                     'impl FileSlotsFields for Wrong {}']:
            self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs', text))

    def test_indirect_producer_accessor_callsite_is_checked(self):
        self.assertTrue(violations('crates/hex-ui/src/arbitrary.rs',
                                   'let fields = staged.sampler_fields_on_control()?;'))

    def test_other_architecture_source_is_unchanged(self):
        self.assertEqual(violations('crates/hex-ui/src/arbitrary.rs',
                                    'use hex_engine::Frame;'), [])


if __name__ == '__main__':
    unittest.main()
