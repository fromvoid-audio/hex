import ast,hashlib,json,pathlib,subprocess
root=pathlib.Path('/workspace/hex');base='65aa67397577aaf304bed83d9757cd443e15245b'
def old(path):return subprocess.check_output(['git','show',base+':'+path],cwd=root,text=True)
def nodes(source):
 t=ast.parse(source)
 return {n.name:ast.get_source_segment(source,n) for n in ast.walk(t) if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef))}
path='scripts/launch-restart.py';before=old(path);after=(root/path).read_text();a=nodes(before);b=nodes(after)
changed={name for name in a if a[name]!=b[name]}
assert changed=={'exact_identity','disk_identity','call','prepare'},changed
assert a['prepare'].replace('    if "HEX_DEV_APP_PATH" in os.environ:\n        raise RestartError("--force-restart requires the repository-configured stable app path")\n','')==b['prepare']
for name in ('_compiler_identity','_file_identity','_check_directory','_run_owned','_validate_helper','_prepare_helper','close','stop_exact','recovery_hash','snapshot','request'):
 assert a[name]==b[name],name
inventory={};old_names=set();new_names=set()
for path in ('scripts/tests/test_launch.py','scripts/tests/test_launch_restart.py','scripts/tests/test_launch_restart_compile.py'):
 def names(source):
  t=ast.parse(source)
  return {path+':'+c.name+'.'+n.name for c in t.body if isinstance(c,ast.ClassDef) for n in c.body if isinstance(n,ast.FunctionDef) and n.name.startswith('test_')}
 o=names(old(path));n=names((root/path).read_text());assert o<=n
 old_names|=o;new_names|=n;inventory[path]={'original':len(o),'final':len(n),'added':sorted(n-o)}
assert len(old_names)==75 and len(new_names)==89
untouched=('launch','scripts/local-ci.py','scripts/hex-test-build.py','scripts/hex-test-build.json')
for path in untouched:assert old(path)==(root/path).read_text(),path
result={'source_parent':base,'changed_old_python_functions':sorted(changed),'new_python_functions':sorted(set(b)-set(a)), 'all_75_original_test_ids_retained':True,'total_unique_test_ids':len(new_names),'tests':inventory,'untouched':list(untouched),'strict_helper_compiler_process_recovery_functions_byte_identical':True,'prepare_only_change_remove_blanket_environment_guard':True}
print(json.dumps(result,indent=2))
