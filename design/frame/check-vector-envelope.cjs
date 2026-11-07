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
frame.validate(doc);
let pointStates = 0, loopStates = 0;
for (const state of Object.keys(doc.states)) {
  const visible = frame.layout(doc, project.viewport, state).filter(node => node.visible);
  const nodes = new Map(visible.map(node => [node.id, node]));
  const amount = nodes.get('filter-envelope-amount');
  assert(amount && /st$/.test(amount.value), state + ': keep semitone depth visible');
  assert(!amount.action, state + ': editing depth must not select an envelope');
  const amountLabel = nodes.get('filter-envelope-amount-label');
  const settings = visible.find(node => node.type === 'button' && node.label === 'device settings');
  assert(amountLabel.bounds.x + amountLabel.bounds.width + 2 <= settings.bounds.x,
    state + ': depth label must stay clear of the settings button');
  const filterSelected = nodes.get('filter-section').style.stroke !== 'none';
  if (filterSelected) {
    assert.equal(nodes.get('adsr-2').value, '50%', state + ': filter sustain uses percent');
    const curve = visible.find(node => /^env-curve-filter-.*-active$/.test(node.id));
    assert(curve);
    assert.equal(curve.points[49][1], 76, state + ': graph sustain matches 50% readout');
  }
  if (state === 'default') assert.equal(amount.value, '0 st');
  if (state === 'filter-depth-positive') assert.equal(amount.value, '+24 st');
  if (state === 'filter-depth-negative') assert.equal(amount.value, '−24 st');
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
