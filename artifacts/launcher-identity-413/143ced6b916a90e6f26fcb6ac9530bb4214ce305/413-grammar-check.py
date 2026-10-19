import ast,pathlib
paths=['launch','scripts/launch-restart.py','scripts/tests/test_launch.py','scripts/tests/test_launch_restart.py','scripts/tests/test_launch_restart_compile.py']
for name in paths:
 source=pathlib.Path(name).read_text()
 ast.parse(source,filename=name,feature_version=(3,9))
 compile(source,name,'exec')
print('5 Python sources: Python3.9 grammar and current-runtime compile passed (not Python3.9 execution)')
