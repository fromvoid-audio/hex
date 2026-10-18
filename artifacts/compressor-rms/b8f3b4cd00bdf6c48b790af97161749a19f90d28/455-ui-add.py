from pathlib import Path
import subprocess
s=subprocess.check_output(['git','show','66ffeab1:crates/hex-ui/src/editor/device_panel/tests/auto_pan_ui_tests.rs'],text=True)
start=s.index('#[gpui::test]\nfn auto_pan_waveform_menu_');end=s.index('\n#[gpui::test]',start+1);s=s[start:end]
s=s.replace('auto_pan_waveform_menu_track_return_cancel_commit_preview_history_and_reopen','compressor_mode_menu_track_return_cancel_commit_preview_history_and_reopen').replace('hex.auto-pan','hex.compressor').replace('auto-pan-waveform-','compressor-transfer-').replace('parameter-4','parameter-8').replace('ParameterId::from_static(4)','ParameterId::from_static(8)').replace('actual waveform options','actual detector mode options').replace('hex-auto-pan-shape-','hex-compressor-mode-')
a=s.index('        editor.update(cx, |view, _| {\n            let device_view =');b=s.index('        editor.update(cx, |view, _| assert!(view.enqueue(Request::Undo)))',a)
s=s[:a]+'''        visual.run_until_parked();
        let mode_label = Box::leak(format!("compressor-rms-detector-{}", device.value()).into_boxed_str());
        assert!(visual.debug_bounds(mode_label).is_some(), "actual RMS detector-axis caption");
        editor.read_with(cx, |view, _| {
            assert_eq!(compressor_panel::cached_curve(view, device), Some(&hex_engine::native_compressor_transfer_curve(changed_state).unwrap()));
            assert_eq!(changed_state.state_version(),3);
            assert_eq!(changed_state.payload().len(),32);
        });
'''+s[b:]
p=Path('crates/hex-ui/src/editor/device_panel/tests/compressor_ui_tests.rs');p.write_text(p.read_text()+'\n'+s+'\n')
p=Path('crates/hex-ui/src/editor/device_panel/compressor_panel.rs');s=p.read_text();a='.child(theme::text(format!(\n                    "static {mode_name} detector -60…0 → -66…24 db"\n                )))';print('caption replace',s.count(a));assert s.count(a)==1;s=s.replace(a,''' .child(div().debug_selector(move || format!("compressor-{mode_name}-detector-{}",device.value())).child(theme::text(format!("static {mode_name} detector -60…0 → -66…24 db"))))''');p.write_text(s)
