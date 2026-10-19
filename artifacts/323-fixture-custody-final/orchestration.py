import importlib.util,pathlib,sys
root=pathlib.Path('/workspace/hex');spec=importlib.util.spec_from_file_location('hex_local_ci',root/'scripts/local-ci.py');m=importlib.util.module_from_spec(spec);sys.modules[spec.name]=m;spec.loader.exec_module(m)
steps=(
 m.Step('divider-paint-fixture',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','divider_paint','--','--nocapture'),uses_cargo_target=True,requires_tests=True),
 m.Step('resize-feedback-controls',('cargo','test','-p','hex-ui','--features','ui-test-support','--locked','resize_feedback','--','--nocapture'),uses_cargo_target=True,requires_tests=True),
 m.Step('ui-feature-strict',('cargo','clippy','-p','hex-ui','--features','ui-test-support','--all-targets','--locked','--','-D','warnings'),uses_cargo_target=True),
 m.Step('format',('cargo','fmt','--all','--check')),
)
raise SystemExit(m.LocalCiRunner(root,mode='component',run_id='323-fixture-custody-final',steps=steps).run())
