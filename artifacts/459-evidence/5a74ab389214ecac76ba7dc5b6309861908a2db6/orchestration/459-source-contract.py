import ast, subprocess
from pathlib import Path
for name in ('scripts/local-ci.py', 'scripts/tests/test_local_ci.py'):
    ast.parse(Path(name).read_text(), feature_version=(3, 9))
name = 'test_real_cargo_worktree_handoff_cleans_only_workspace_dev_artifacts'
def body(source):
    node = next(n for n in ast.walk(ast.parse(source)) if isinstance(n, ast.FunctionDef) and n.name == name)
    return '\n'.join(source.splitlines()[min(d.lineno for d in node.decorator_list) - 1:node.end_lineno])
old = subprocess.check_output(['git', 'show', '2640b0ee:scripts/tests/test_local_ci.py'], text=True)
assert body(old) == body(Path('scripts/tests/test_local_ci.py').read_text())
print('Python3.9 grammar and original real-handoff test byte preservation PASS')
