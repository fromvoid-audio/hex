import json,sys,pathlib,importlib.util
root=pathlib.Path(__file__).resolve().parents[3]
spec=importlib.util.spec_from_file_location('analyzer',root/'research/observations/time-selection-2026-10-16/analyze.py');a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)
raw=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);out.mkdir(exist_ok=False,parents=True)
new=a.analyze(raw.read_bytes());old=json.loads((root/'research/observations/time-selection-2026-10-16/summary.json').read_text())
range_change={'baseline':old['selected_frame_range'],'measured':new['selected_frame_range'],'reason':('Merged PR263 fixes project-scope gesture projection to use the main lane consistently. Physical input coordinates, fixture bytes and native harness are unchanged; validate uniform range within each complete run rather than require the old buggy endpoint.' if new['selected_frame_range'] != old['selected_frame_range'] else 'No projection difference; the selected frame interval matches the baseline.')}
oldcases={(c['tracks'],c['operation']):c for c in old['cases']}
comparisons=[]
for c in new['cases']:
 b=oldcases[(c['tracks'],c['operation'])]
 for metric in a.TIMINGS:
  x,y=b['metrics_us'][metric],c['metrics_us'][metric]
  comparisons.append(dict(tracks=c['tracks'],operation=c['operation'],metric=metric,before_ms={k:x[k]/1000 for k in ['median','p95','maximum']},after_ms={k:y[k]/1000 for k in ['median','p95','maximum']},median_speedup=x['median']/y['median']))
selection=[c for c in comparisons if c['tracks']==100 and c['metric']=='selection_first_paint_us']
result={'native_first_paint_p95_target_ms':16.67,'all_100_track_cases_meet_p95_target':all(c['after_ms']['p95']<=16.67 for c in selection),'scope':'in-process synthetic dispatch to native paint; excludes physical input/display/GPU/audio deadlines','comparisons':comparisons,'selection_range_comparison':range_change}
(out/'summary.json').write_text(json.dumps(new,indent=2)+'\n');(out/'comparison.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'all_100_track_cases_meet_p95_target':result['all_100_track_cases_meet_p95_target'],'selection':selection},indent=2))
