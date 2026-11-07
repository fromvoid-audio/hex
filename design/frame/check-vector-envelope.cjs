// Focused layout regression check for Vector's envelope study.
// Usage: node design/frame/check-vector-envelope.cjs /absolute/path/to/frame
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
if (!process.argv[2]) throw new Error('pass the installed Frame runtime directory');
const frame = require(path.join(path.resolve(process.argv[2]), 'design-document.js'));
const project = JSON.parse(fs.readFileSync(path.join(__dirname, 'vector-device.frame.json')));
const doc = project.archive.revisions[0].document;
const started = performance.now();
const near = (actual, expected, message) => assert(Math.abs(actual - expected) < 1e-8, message);
frame.validate(doc);
let pointStates = 0, loopStates = 0;
for (const state of Object.keys(doc.states)) {
  const visible = frame.layout(doc, project.viewport, state).filter(node => node.visible);
  const nodes = new Map(visible.map(node => [node.id, node]));
  const voice = visible.filter(node => node.label === 'choose voice count');
  assert.equal(voice.length, 1, state + ': one voice-count dropdown');
  assert(!visible.some(node => node.label === 'toggle mono poly' || node.id === 'setting-value-0'), state + ': voice control is not duplicated');
  for (const id of ['initial', 'peak', 'a-curve', 'd-curve', 'r-curve'])
    assert(nodes.has('env-direct-' + id), state + ': expose envelope shaping directly');
  for (const op of ['a', 'b', 'c', 'd']) {
    assert(nodes.has('level-fill-' + op) && nodes.has('level-well-' + op), state + ': filled oscillator level slider');
    assert(!nodes.has('level-knob-' + op) && !nodes.has('level-arc-' + op), state + ': no oscillator level knob');
    const slider = nodes.get('level-' + op);
    assert.equal(slider.bounds.y, nodes.get('ratio-' + op).bounds.y, state + ': tuning and level align');
    const wave = visible.find(node => node.label === 'choose oscillator ' + op + ' waveform');
    assert.equal(wave.bounds.y, nodes.get('hz-' + op).bounds.y, state + ': frequency and waveform align');
  }
  const amount = nodes.get('filter-envelope-amount');
  assert(amount && (state === 'filter-depth-edit' ? /st$/.test(amount.value) : /hz$/.test(amount.value)),
    state + ': display peak frequency normally and semitones during exact entry');
  assert(!amount.action, state + ': editing depth must not select an envelope');
  const amountLabel = nodes.get('filter-envelope-amount-label');
  assert(!visible.some(node => node.type === 'button' && node.label === 'device settings' && node.parentId === 'core-controls'),
    state + ': device settings belong in the host menu');
  const filterBounds = nodes.get('filter-section').bounds;
  for (const id of ['cutoff', 'resonance', 'filter-envelope-amount']) {
    const bounds = nodes.get(id).bounds;
    assert(bounds.x >= filterBounds.x && bounds.x + bounds.width <= filterBounds.x + filterBounds.width &&
      bounds.y >= filterBounds.y && bounds.y + bounds.height <= filterBounds.y + filterBounds.height, state + ': group all filter controls');
  }
  for (const op of ['a', 'b', 'c', 'd']) {
    const identity = nodes.get('osc-identity-' + op);
    assert(identity && identity.type === 'text' && identity.text === op && !identity.action, state + ': noninteractive oscillator identity');
  }
  const filterSelected = nodes.get('filter-section').style.stroke !== 'none';
  if (filterSelected) {
    assert.equal(nodes.get('adsr-2').value, '50%', state + ': filter sustain uses percent');
    const curve = visible.find(node => /^env-curve-filter-.*-active$/.test(node.id));
    assert(curve);
    near(curve.points[49][1], (nodes.get('env-bg').bounds.y + curve.points[0][1]) / 2, state + ': graph sustain matches 50% readout');
  }
  if (state === 'default') assert.equal(amount.value, nodes.get('cutoff').value);
  if (state === 'filter-depth-positive') assert.equal(amount.value, '2 khz');
  if (state === 'filter-depth-negative') assert.equal(amount.value, '125 hz');
  if (state === 'device-menu') {
    const menuItems = visible.filter(node => node.type === 'button' && node.parentId === 'device-popup');
    assert.equal(new Set(menuItems.map(node => node.bounds.x)).size, 1, 'preset menu items align');
    assert.equal(new Set(menuItems.map(node => node.bounds.width)).size, 1, 'preset menu items share a width');
    assert.equal(new Set(menuItems.map(node => node.bounds.height)).size, 1, 'preset menu items share a height');
  }
  const readout = nodes.get('selected-level');
  if (readout) {
    const title = nodes.get('envelope-title');
    assert(title, state + ': selected point must have a name');
    assert(title.bounds.x + title.bounds.width + 6 <= readout.bounds.x,
      state + ': point name and value must have separate reserved areas');
    assert(title.text.length * title.style.fontSize * 0.6 <= title.bounds.width,
      state + ': point name must fit its monospace area');
    if (filterSelected && /initial|peak|decay-sustain/.test(state))
      assert(/%/.test(readout.value) && !/db|—/.test(readout.value), state + ': normalized filter point readout');
    assert(/^(a|b|c|d|f) \/ /.test(title.text), state + ': retain source identity while editing a point');
    assert(readout.style.fontSize >= 9, state + ': keep exact values readable');
    pointStates++;
  }
  const loop = nodes.get('loop-visual');
  const held = nodes.get('loop-held-region');
  const bracket = nodes.get('loop-repeat-bracket');
  if (loop) {
    assert(held && bracket && nodes.get('loop-repeat-label'), state + ': show repetition while held');
    const noteOff = visible.find(node => node.id.endsWith('-noteoff'));
    const cycle = visible.find(node => node.id.endsWith('-loop-shade'));
    const release = visible.find(node => /^release-(a|b|c|d|filter)-/.test(node.id));
    const noteOffLabel = visible.find(node => node.id.endsWith('-release-label'));
    assert(noteOff && cycle && release && noteOffLabel);
    const active = visible.find(node => /^env-curve-(a|b|c|d|filter)-.*-active$/.test(node.id));
    assert(active && active.points.length > 75, state + ': show a second attack/decay cycle');
    near(active.points[50][0], active.points[49][0], state + ': reset adds no cycle time');
    assert.equal(active.points[50][1], active.points[0][1], state + ': reset reaches initial level');
    assert.equal(active.points[75][1], active.points[24][1], state + ': repeated attack reaches the same peak');
    const reset = visible.find(node => node.id.endsWith('-loop-end'));
    assert(reset && reset.points.length > 2, state + ': show the reset arrow');
    near(reset.bounds.x + reset.points[0][0], active.bounds.x + active.points[49][0], state + ': reset starts at decay end');
    near(reset.bounds.y + reset.points[0][1], active.bounds.y + active.points[49][1], state + ': reset starts at sustain level');
    near(reset.bounds.y + reset.points[1][1], active.bounds.y + active.points[0][1], state + ': reset ends at initial level');
    assert(nodes.get('loop-repeat-label').text.includes('reset'));
    if (/-tempo-|-pitch-/.test(active.id))
      assert(Math.abs(release.points[0][1] - active.points[49][1]) < 1e-8, state + ': note-off wins over boundary reset');
    const releaseHandle = visible.find(node => node.label === 'edit release curve envelope');
    assert(Math.abs(releaseHandle.bounds.y + releaseHandle.bounds.height / 2 -
      (release.bounds.y + release.points[12][1])) < 1e-8, state + ': release handle follows current level');
    const toSeconds = x => Math.expm1((x - 6) / nodes.get('env-bg').bounds.width * Math.log1p(140 / 0.001)) * 0.001;
    const cycleTime = toSeconds(active.points[49][0]);
    const attackTime = toSeconds(active.points[24][0]);
    const offTime = toSeconds(noteOff.bounds.x - active.bounds.x);
    const turns = offTime / cycleTime;
    const phase = Math.abs(turns - Math.round(turns)) < 1e-8 ? cycleTime : offTime % cycleTime;
    const expected = phase <= attackTime
      ? active.points[0][1] + (active.points[24][1] - active.points[0][1]) * Math.pow(phase / attackTime, 0.7)
      : active.points[24][1] + (active.points[49][1] - active.points[24][1]) *
        Math.pow((phase - attackTime) / (cycleTime - attackTime), 2);
    assert(Math.abs(release.points[0][1] - expected) < 1e-7, state + ': release begins at the repeated shape’s current level');
    assert.equal(noteOffLabel.text, 'note off', state + ': identify the loop exit');
    near(held.bounds.x + held.bounds.width, noteOff.bounds.x,
      state + ': held region ends exactly at note-off');
    near(bracket.bounds.x + bracket.bounds.width, noteOff.bounds.x,
      state + ': repetition bracket ends exactly at note-off');
    assert(cycle.bounds.width <= held.bounds.width, state + ': distinguish one cycle from held duration');
    near(release.bounds.x + release.points[0][0], noteOff.bounds.x,
      state + ': release starts at note-off');
    const releaseLabel = nodes.get('loop-release-label');
    assert(releaseLabel && releaseLabel.text === 'release', state + ': identify release separately');
    assert(noteOffLabel.bounds.y + noteOffLabel.bounds.height <= releaseLabel.bounds.y,
      state + ': note-off and release labels must not collide');
    loopStates++;
  } else {
    assert(!held && !bracket && !nodes.get('loop-repeat-label') && !nodes.get('loop-release-label'),
      state + ': loop-only guidance must disappear when looping is off');
  }
}
const routingNodes = doc.elements.filter(node => /^(algorithm-diagram|routing-choice)-(serial|paired|parallel)-node-[abcd]$/.test(node.id));
assert.equal(routingNodes.length, 24);
for (const node of routingNodes) {
  const op = node.id.at(-1);
  assert(node.type === 'container' && !node.text && node.bounds.width === 8 && node.bounds.height === 8, 'routing uses small squares without letters');
  assert.equal(node.style.fill, doc.elements.find(element => element.id === 'ratio-' + op).style.color, 'routing colors match their oscillators');
}
assert(doc.elements.findIndex(node => node.id === 'routing-popup') >
  doc.elements.findIndex(node => node.id === 'filter-envelope-amount'), 'routing popup paints above filter fields');
const half = frame.layout(doc, project.viewport, 'filter-peak-half').filter(node => node.visible);
assert.equal(half.find(node => node.id === 'filter-envelope-amount').value, '1 khz');
assert.equal(half.find(node => node.id === 'selected-level').value, '50%');
const editing = frame.layout(doc, project.viewport, 'filter-depth-edit').filter(node => node.visible);
assert.equal(editing.find(node => node.id === 'filter-envelope-amount').value, '+24 st');
const hint = editing.find(node => node.id === 'filter-depth-edit-hint');
assert(hint);
const glideToggle = editing.find(node => node.label === 'toggle glide');
assert(hint.bounds.y + hint.bounds.height + 2 <= glideToggle.bounds.y, 'exact-entry hint stays clear of glide');
for (const [state, id] of [
  ['focused-section-b', 'row-b'],
  ['focused-routing', 'algorithm-diagram-serial-picker-surface'],
  ['focused-routing-choice', 'routing-choice-paired-picker-surface'],
  ['focused-env-point', 'env-keyboard-focus-ring'],
]) {
  const target = frame.layout(doc, project.viewport, state).find(node => node.visible && node.id === id);
  assert(target && target.style.stroke === '#eeeeee', state + ': focus marks its intended control');
}
const slots = frame.layout(doc, project.viewport, 'slots-4').filter(node => node.visible);
for (const value of slots.filter(node => /^macro-slots-4-value-/.test(node.id))) {
  const knob = slots.find(node => node.id === value.id.replace('-value-', '-knob-'));
  assert(value.style.fontSize >= 9 && value.bounds.x >= knob.bounds.x + knob.bounds.width, 'macro values stay beside knobs');
}
for (const [state, expectedWidth] of [['default', 620], ['settings', 788], ['a-options', 788], ['a-harmonics', 884]]) {
  const nodes = frame.layout(doc, project.viewport, state).filter(node => node.visible);
  const meter = nodes.find(node => node.id === 'output-meter');
  near(meter.bounds.x + meter.bounds.width, expectedWidth, state + ': content-sized device width');
  for (const field of nodes.filter(node => node.type === 'input' && ['osc-options', 'settings-panel'].includes(node.parentId)))
    assert(field.bounds.width <= 54 && field.style.fontSize >= 10, state + ': compact settings keep readable values');
  if (['settings', 'a-options'].includes(state)) {
    assert(nodes.some(node => node.id === 'compact-pane'));
    assert(!nodes.some(node => node.id === 'optional-pane'));
    assert(nodes.some(node => node.parentId === 'compact-pane' && node.label === 'close optional panel'));
    assert(doc.elements.findIndex(node => node.id === 'skin-compact-options') < doc.elements.findIndex(node => node.id === 'row-a'), 'host skin stays behind controls');
  }
}
const pointWidth = doc.elements.find(node => node.id === 'selected-level');
assert('999ms/-35.55db'.length * pointWidth.style.fontSize * 0.6 + 2 * pointWidth.style.padding <= pointWidth.bounds.width, 'compact readout fits fine sustain and time');
for (const id of ['cutoff', 'filter-envelope-amount']) {
  const field = doc.elements.find(node => node.id === id);
  assert('19.99 khz'.length * field.style.fontSize * 0.6 + 2 * field.style.padding <= field.bounds.width, 'frequency field fits its formatted range');
}
const core = frame.layout(doc, project.viewport, 'default').filter(node => node.visible);
const fields = ['feedback', 'cutoff', 'resonance', 'filter-envelope-amount'].map(id => core.find(node => node.id === id));
assert.equal(new Set(fields.map(node => node.bounds.x)).size, 1, 'right-side parameter values share one column');
assert(fields.every(node => node.bounds.width === 60 && node.bounds.height === 18), 'right-side parameter values share dimensions');
assert.equal(core.find(node => node.label === 'choose voice count').bounds.y, core.find(node => node.id === 'glide').bounds.y, 'voicing controls share a bottom baseline');
for (const count of [1, 2, 4, 8, 16, 32]) {
  const state = count === 8 ? 'voices-menu' : 'voices-menu-' + count;
  const nodes = frame.layout(doc, project.viewport, state).filter(node => node.visible);
  assert(nodes.some(node => node.id === 'voice-count-menu'));
  assert.equal(nodes.find(node => node.label === 'choose voice count').text, count + ' ▾');
  assert.equal(nodes.find(node => node.id === 'voice-choice-' + count).style.fill, '#2a3c4c');
  assert.equal(nodes.find(node => node.id === 'voice-choice-' + count).action.state, 'voices-' + count);
}
assert(pointStates >= 35 && loopStates >= 15);
console.log(`${pointStates} point states and ${loopStates} loop states passed in ${((performance.now()-started)/1000).toFixed(2)} s`);
