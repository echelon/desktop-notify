import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const notification = (id = 'a', extra = {}) => ({ id, session_id: `session-${id}`, title: 'Task', message: 'Question?', kind: 'awaiting_user_input', origin: { terminal_app: 'ghostty' }, silenced: false, ...extra });

class Element {
  constructor(tag = 'div') { this.tagName = tag.toUpperCase(); this.className = ''; this.dataset = {}; this.children = []; this.handlers = {}; this.classList = { toggle() {} }; }
  addEventListener(event, handler) { this.handlers[event] = handler; }
  setAttribute(name, value) { this[name] = value; }
  remove() { if (this.parent) { this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; } }
  append(...nodes) { nodes.forEach((node) => this.insertBefore(node, null)); }
  insertBefore(node, before) { node.remove(); this.children.splice(before ? this.children.indexOf(before) : this.children.length, 0, node); node.parent = this; }
  find(className) { if (this.className.split(' ').includes(className)) return this; return this.children.map((child) => child.find(className)).find(Boolean); }
}

async function ui(alerts, invoke = async () => ({ target: 'application' }), sound = { alerting: true }) {
  const elements = new Map();
  const calls = [];
  let update;
  const element = (id) => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
  const context = {
    Date, setInterval() {},
    document: { createElement: (tag) => new Element(tag), getElementById: element, addEventListener() {} },
    window: { __TAURI__: {
      core: { async invoke(command, args) {
        calls.push([command, args]);
        if (command === 'get_snapshot') return { notifications: alerts, sound, connected: true, error: null };
        return invoke(command, args);
      } },
      event: { async listen(_event, handler) { update = handler; } },
      window: { getCurrentWindow() { return {}; } },
    } },
  };
  vm.runInNewContext(source, context);
  await new Promise(setImmediate);
  return {
    element, calls,
    row: (id) => element('notifications').children.find((row) => row.dataset.id === id),
    update: (notifications, connected = true, nextSound = sound) => update({ payload: { notifications, sound: nextSound, connected, error: null } }),
  };
}

test('independent sessions show their own status and focus metadata', async () => {
  const page = await ui([notification(), notification('b', { kind: 'all_tasks_finished', origin: null })]);
  assert.equal(page.element('notifications').children.length, 2);
  assert.equal(page.row('a').find('kind').textContent, 'Input needed');
  assert.equal(page.row('b').find('kind').textContent, 'Finished');
  assert.equal(page.row('a').find('focus').disabled, false);
  assert.equal(page.row('b').find('focus').disabled, true);
  assert.match(page.row('b').find('focus').title, /No terminal information/);
  assert.notEqual(page.row('a').find('session').textContent, page.row('b').find('session').textContent);
});

test('Focus leaves all rows available for global sound stopping and clearing', async () => {
  const page = await ui([notification(), notification('b')]);
  await page.row('b').find('focus').handlers.click();
  assert.equal(page.calls.find(([name]) => name === 'focus_notification')[1].id, 'b');
  assert.ok(!page.calls.some(([name]) => name === 'hide_window' || name === 'dismiss_notification'));
  assert.equal(page.row('b').find('silence'), undefined);
  assert.equal(page.element('sound-controls').hidden, false);
  assert.equal(page.element('silence-all').disabled, false);
  await page.element('silence-all').handlers.click();
  assert.deepEqual(page.calls.at(-1), ['silence_all_sound', undefined]);
  page.update([notification('a', { silenced: true }), notification('b', { silenced: true })], true, { alerting: false });
  assert.equal(page.row('a').find('row-sound').hidden, false);
  assert.equal(page.element('silence-all').disabled, true);
  assert.equal(page.element('sound-status').textContent, 'All sounds stopped');
  await page.row('b').find('clear').handlers.click();
  assert.equal(page.calls.find(([name]) => name === 'dismiss_notification')[1].id, 'b');
  page.update([notification()]);
  assert.equal(page.element('notifications').children.length, 1);
  assert.equal(page.element('empty').hidden, true);
  assert.ok(page.row('a'));
});

test('focus errors and expanded messages survive updates to other rows', async () => {
  const page = await ui([notification(), notification('b')], async (command) => {
    if (command === 'focus_notification') throw new Error('App exited');
  });
  const a = page.row('a');
  a.find('details').open = true;
  await a.find('focus').handlers.click();
  page.update([notification('c', { session_id: 'session-b' }), notification()]);
  assert.equal(page.row('a'), a);
  assert.equal(a.find('details').open, true);
  assert.match(a.find('focus-status').textContent, /App exited/);
  assert.equal(a.find('focus-status').hidden, false);
  assert.equal(page.row('c').find('focus-status').hidden, true);
});

test('pending Focus is scoped to one row and cannot affect its replacement', async () => {
  let resolve;
  const page = await ui([notification(), notification('b')], (command) => {
    if (command === 'focus_notification') return new Promise((r) => { resolve = r; });
  });
  const pending = page.row('a').find('focus').handlers.click();
  assert.equal(page.row('a').find('focus').disabled, true);
  assert.equal(page.row('b').find('focus').disabled, false);
  page.update([notification('c', { session_id: 'session-a' }), notification('b')]);
  resolve({ target: 'application' });
  await pending;
  assert.equal(page.row('c').find('focus-status').hidden, true);
  assert.equal(page.row('c').find('focus').disabled, false);
  assert.ok(!page.calls.some(([name]) => name === 'hide_window'));
});

test('clear failures stay on their row and offline updates preserve both rows', async () => {
  const page = await ui([notification(), notification('b')], async (command) => {
    if (command === 'dismiss_notification') throw new Error('Service offline');
  });
  await page.row('b').find('clear').handlers.click();
  assert.match(page.row('b').find('focus-status').textContent, /Service offline/);
  assert.equal(page.row('a').find('focus-status').hidden, true);
  page.update([notification(), notification('b')], false);
  assert.equal(page.row('a').find('clear').disabled, true);
  assert.equal(page.element('silence-all').disabled, true);
  assert.equal(page.element('snooze-5').disabled, true);
  assert.equal(page.row('a').find('focus').disabled, false);
  page.update([]);
  assert.equal(page.element('empty').hidden, false);
});

test('context adds a compact project and task preview with full optional details', async () => {
  const context = { cwd: '/workspace/notifier', repo_name: 'Desktop Notify', repo_description: 'Agent notifications.', current_ask: 'Add context fields', work_arc: 'Make several agents easier to follow' };
  const page = await ui([notification('a', { title: 'notifier: Which layout?', context })]);
  const row = page.row('a');
  assert.equal(row.find('project').textContent, 'Desktop Notify');
  assert.equal(row.find('task-title').textContent, 'Which layout?');
  assert.equal(row.find('context-preview').textContent, 'Task · Add context fields');
  assert.equal(row.find('summary').textContent, 'Details');
  assert.ok(!row.find('details').open);
  for (const [field, value] of Object.entries(context)) {
    assert.equal(row.find(`context-${field}`).hidden, false);
    assert.equal(row.find(`context-${field}`).find('context-value').textContent, value);
  }
});

test('partial and absent context fall back without empty labels or invented repo names', async () => {
  const page = await ui([notification(), notification('b', { context: { cwd: '/workspace/example/', work_arc: 'Ship the app' } })]);
  assert.equal(page.row('a').find('project').hidden, true);
  assert.equal(page.row('a').find('context-preview').hidden, true);
  assert.equal(page.row('a').find('session-context').hidden, true);
  assert.equal(page.row('a').find('summary').textContent, 'View message');
  assert.equal(page.row('a').find('task-title').textContent, 'Task');
  assert.equal(page.row('b').find('project').textContent, 'example');
  assert.equal(page.row('b').find('context-preview').textContent, 'Work · Ship the app');
  assert.equal(page.row('b').find('context-repo_description').hidden, true);
  assert.equal(page.row('b').find('context-current_ask').hidden, true);
});

test('context changes and literal markup update safely without collapsing details', async () => {
  const page = await ui([notification('a', { context: { work_arc: 'Earlier work' } })]);
  const row = page.row('a');
  row.find('details').open = true;
  const literal = '<img src=x onerror="alert(1)">';
  page.update([notification('a', { context: { repo_name: literal, work_arc: '', current_ask: literal } })]);
  assert.equal(row.find('details').open, true);
  assert.equal(row.find('project').textContent, literal);
  assert.equal(row.find('context-current_ask').find('context-value').textContent, literal);
  assert.equal(row.find('context-work_arc').hidden, true);
});

test('snooze buttons request fixed durations and show a countdown to the recorded deadline', async () => {
  const page = await ui([notification()]);
  assert.equal(page.element('sound-status').textContent, 'Sound on');
  await page.element('snooze-1').handlers.click();
  assert.equal(JSON.stringify(page.calls.at(-1)), '["snooze_sound",{"seconds":60}]');
  await page.element('snooze-5').handlers.click();
  assert.equal(JSON.stringify(page.calls.at(-1)), '["snooze_sound",{"seconds":300}]');
  const until = new Date(Date.now() + 272_500).toISOString();
  page.update([notification()], true, { alerting: true, snoozed_until: until });
  assert.equal(page.element('sound-status').textContent, 'Snoozed · resumes in 4:33');
  assert.equal(page.element('silence-all').disabled, false);
  // An elapsed deadline no longer reads as snoozed while the service catches up.
  page.update([notification()], true, { alerting: true, snoozed_until: new Date(Date.now() - 1000).toISOString() });
  assert.equal(page.element('sound-status').textContent, 'Sound on');
  // A snooze stays visible even after every row has been cleared.
  page.update([], true, { alerting: false, snoozed_until: until });
  assert.equal(page.element('sound-controls').hidden, false);
  page.update([], true, { alerting: false });
  assert.equal(page.element('sound-controls').hidden, true);
});

test('sound failures surface in the sound bar without touching rows', async () => {
  const page = await ui([notification()], async (command) => {
    if (command === 'snooze_sound') throw new Error('Service offline');
  });
  await page.element('snooze-1').handlers.click();
  assert.match(page.element('sound-status').textContent, /Service offline/);
  assert.equal(page.row('a').find('focus-status').hidden, true);
  assert.equal(page.element('snooze-1').disabled, false);
});
