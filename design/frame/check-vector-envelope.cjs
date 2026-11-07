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
  const amount = nodes.get('filter-envelope-amount');
  assert(amount && /hz$/.test(amount.value), state + ': display the peak destination frequency');
  assert(!amount.action, state + ': editing depth must not select an envelope');
  const amountLabel = nodes.get('filter-envelope-amount-label');
  const settings = visible.find(node => node.type === 'button' && node.label === 'device settings' && node.parentId === 'core-controls');
  assert(amountLabel.bounds.x + amountLabel.bounds.width + 2 <= settings.bounds.x,
    state + ': depth label must stay clear of the settings button');
  const filterSelected = nodes.get('filter-section').style.stroke !== 'none';
  if (filterSelected) {
    assert.equal(nodes.get('adsr-2').value, '50%', state + ': filter sustain uses percent');
    const curve = visible.find(node => /^env-curve-filter-.*-active$/.test(node.id));
    assert(curve);
    assert.equal(curve.points[49][1], 76, state + ': graph sustain matches 50% readout');
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
    assert(nodes.get('loop-repeat-label').text.includes('reset to initial'));
    if (/-tempo-|-pitch-/.test(active.id))
      assert(Math.abs(release.points[0][1] - active.points[49][1]) < 1e-8, state + ': note-off wins over boundary reset');
    const releaseHandle = visible.find(node => node.label === 'edit release curve envelope');
    assert(Math.abs(releaseHandle.bounds.y + releaseHandle.bounds.height / 2 -
      (release.bounds.y + release.points[12][1])) < 1e-8, state + ': release handle follows current level');
    const toSeconds = x => Math.expm1((x - 6) / 280 * Math.log1p(140 / 0.001)) * 0.001;
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
    assert.equal(held.bounds.x + held.bounds.width, noteOff.bounds.x,
      state + ': held region ends exactly at note-off');
    assert.equal(bracket.bounds.x + bracket.bounds.width, noteOff.bounds.x,
      state + ': repetition bracket ends exactly at note-off');
    assert(cycle.bounds.width <= held.bounds.width, state + ': distinguish one cycle from held duration');
    assert.equal(release.bounds.x + release.points[0][0], noteOff.bounds.x,
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
assert(pointStates >= 35 && loopStates >= 15);
console.log(`${pointStates} point states and ${loopStates} loop states passed in ${((performance.now()-started)/1000).toFixed(2)} s`);
