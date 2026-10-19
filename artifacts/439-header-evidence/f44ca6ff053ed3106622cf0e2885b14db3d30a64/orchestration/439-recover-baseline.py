import subprocess,io,tarfile,hashlib,json
from pathlib import Path
base='fb4cb8be3896b73c8ebda76027bbcbfbd02622ec';name='crates/hex-ui/src/editor/track_gain/tests.rs'
addition='''
#[gpui::test]
fn header_volume_is_one_filled_numeric_field_not_a_separate_rail(cx: &mut TestAppContext) {
    let (_browser, mut visual, _editor, [track, _]) = actor_fixture(cx);
    let bounds = |visual: &mut VisualTestContext, suffix: &str| {
        visual.debug_bounds(Box::leak(format!("track-volume-{}{suffix}", track.value()).into_boxed_str())).unwrap()
    };
    let whole = bounds(&mut visual, "");
    let field = bounds(&mut visual, "-value");
    assert!(field.size.width >= whole.size.width - px(4.0), "reference requires a centered value across the filled bar, not a separate narrow value field: {whole:?} {field:?}");
    assert_eq!(field.center().x, whole.center().x);
    assert!(field.size.height >= px(16.0));
}
'''
tf=tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git','archive',base])))
files={m.name:(m,tf.extractfile(m).read() if m.isfile() else None) for m in tf.getmembers() if not m.isdir()};original=files[name][1]
files[name]=(files[name][0],original+addition.encode())
h=hashlib.sha256()
for path,(m,data) in sorted(files.items()):
 p=path.encode();h.update(len(p).to_bytes(8,'big'));h.update(p)
 if m.issym():h.update(b'symlink\0');h.update(m.linkname.encode())
 elif m.isfile():h.update(b'file\0');h.update(data);h.update(str((Path('/workspace/hex')/path).lstat().st_mode&0o111).encode())
 else:raise AssertionError(path)
expected=json.load(open('artifacts/local-ci/header-width-return-439-baseline/summary.json'))['start_tree']['content_sha256']
print(h.hexdigest(),expected);assert h.hexdigest()==expected
p=Path('artifacts/439-baseline-recovered');p.mkdir(exist_ok=True);(p/'tests.rs').write_bytes(files[name][1]);(p/'baseline.patch').write_text(''.join(__import__('difflib').unified_diff(original.decode().splitlines(True),files[name][1].decode().splitlines(True),fromfile='a/'+name,tofile='b/'+name)))
(p/'PROOF.json').write_text(json.dumps({'base':base,'content_sha256':expected,'exact_initial_test_bytes_recovered_and_full_repository_digest_matches':True,'changed_only':name,'test_file_sha256':hashlib.sha256(files[name][1]).hexdigest()},indent=2)+'\n')
