from pathlib import Path
import json,hashlib,tempfile,subprocess,difflib,re,shutil
r=Path('/workspace/hex');v=r/'third_party/gpui';base='0bd10829f16c9de92aafb9082373dc154c92aa3e'
def gitshow(path):return subprocess.check_output(['git','show',base+':third_party/gpui/'+path],cwd=r)
def sha(b):return hashlib.sha256(b).hexdigest()
old=json.loads(gitshow('hex-provenance.json'));manifest=json.loads((v/'hex-provenance.json').read_text());assert old['original_files']==manifest['original_files'] and old.get('added_files')==manifest.get('added_files')
oldpatch=gitshow('hex-patches.diff');tmp=Path(tempfile.mkdtemp(prefix='hex407-verified-gpui-'));patched=tmp/'patched';patched.mkdir()
for name in list(old['patched_files'])+list(old.get('added_files',{})):
 b=gitshow(name);t=patched/name;t.parent.mkdir(parents=True,exist_ok=True);t.write_bytes(b)
p=tmp/'old.diff';p.write_bytes(oldpatch);subprocess.run(['git','apply','--reverse','--unidiff-zero','--whitespace=nowarn',str(p)],cwd=patched,check=True)
for name in old['patched_files']:assert sha((patched/name).read_bytes())==old['original_files'][name],name
for name in old.get('added_files',{}):assert not (patched/name).exists(),name
changed={name for name,prior in old['patched_files'].items() if sha((v/name).read_bytes())!=prior}
assert changed=={'src/interactive.rs','src/platform/mac/events.rs','src/platform/windows/events.rs','src/platform/linux/wayland/client.rs','src/platform/linux/x11/client.rs'},changed
blocks=[]
for block in re.split(r'(?m)(?=^diff --git )',oldpatch.decode()):
 if not block:continue
 m=re.match(r'diff --git a/(.*?) b/(.*?)\n',block);assert m and m[1]==m[2];name=m[1]
 if name in changed:
  original=(patched/name).read_text();current=(v/name).read_text();block='diff --git a/'+name+' b/'+name+'\n'+''.join(difflib.unified_diff(original.splitlines(True),current.splitlines(True),fromfile='a/'+name,tofile='b/'+name,n=0))
 blocks.append(block)
newpatch=''.join(blocks);(v/'hex-patches.diff').write_text(newpatch)
for name in changed:manifest['patched_files'][name]=sha((v/name).read_bytes())
assert old['original_files']==manifest['original_files'] and old['crates_io_sha256']==manifest['crates_io_sha256']
(v/'hex-provenance.json').write_text(json.dumps(manifest,indent=2)+'\n')
p=tmp/'new.diff';p.write_text(newpatch);subprocess.run(['git','apply','--check','--unidiff-zero',str(p)],cwd=patched,check=True);subprocess.run(['git','apply','--unidiff-zero',str(p)],cwd=patched,check=True)
for name in list(old['patched_files'])+list(old.get('added_files',{})):assert (patched/name).read_bytes()==(v/name).read_bytes(),name
proof={'baseline_source':base,'archive_sha256_unchanged':manifest['crates_io_sha256'],'original_source_hashes_verified':len(old['patched_files']),'cumulative_patched_and_added_bytes_verified':len(old['patched_files'])+len(old.get('added_files',{})),'changed_existing_files':{n:{'before_sha256':old['patched_files'][n],'after_sha256':manifest['patched_files'][n]} for n in sorted(changed)},'unchanged_patch_blocks_retained':True,'guards_unchanged':True}
(r/'artifacts/407-four-returns/gpui-provenance-proof.json').write_text(json.dumps(proof,indent=2)+'\n');print(json.dumps(proof,indent=2));shutil.rmtree(tmp)
