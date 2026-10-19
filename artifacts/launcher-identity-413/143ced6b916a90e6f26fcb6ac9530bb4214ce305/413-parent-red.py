import sys,types,subprocess,unittest
sys.path.insert(0,'/workspace/hex/scripts/tests')
import test_launch_restart as cases
source=subprocess.check_output(['git','show','65aa67397577aaf304bed83d9757cd443e15245b:scripts/launch-restart.py'],text=True)
old=types.ModuleType('restart_parent_65aa');sys.modules[old.__name__]=old
exec(compile(source,'65aa:scripts/launch-restart.py','exec'),old.__dict__)
cases.restart=old
suite=unittest.TestSuite([
 cases.FilesystemIdentityTests('test_actual_target_pins_and_observed_strings_are_preserved'),
 cases.PreflightTests('test_supported_configured_override_uses_same_verified_preflight'),
])
result=unittest.TextTestRunner(verbosity=2).run(suite)
raise SystemExit(not result.wasSuccessful())
