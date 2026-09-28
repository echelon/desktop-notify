import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const notification = (id = 'a', extra = {}) => ({ id, session_id: `session-${id}`, title: 'Task', message: 'Question?', state: 'input_needed', origin: { terminal_app: 'ghostty' }, ...extra });

class Element {
  constructor(tag = 'div') { this.tagName = tag.toUpperCase(); this.className = ''; this.dataset = {}; this.children = []; this.handlers = {}; this.classList = { toggle() {} }; }
  addEventListener(event, handler) { this.handlers[event] = handler; }
  setAttribute(name, value) { this[name] = value; }
  remove() { if (this.parent) { this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; } }
  append(...nodes) { nodes.forEach((node) => this.insertBefore(node, null)); }
  replaceChildren(...nodes) { [...this.children].forEach((child) => child.remove()); this.append(...nodes); }
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
    document: { createElement: (tag) => new Element(tag), createElementNS: (_ns, tag) => new Element(tag), getElementById: element, addEventListener() {} },
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
    update: (notifications, connected = true, nextSound = sound) => update({ payload: { notifications, sound: nextSound, service: 'http://127.0.0.1:43110', connected, error: null } }),
  };
}

test('independent sessions show their own status and focus metadata', async () => {
  const page = await ui([notification(), notification('b', { state: 'done', origin: null })]);
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
  assert.deepEqual(page.calls.at(-1), ['stop_all_sound', undefined]);
  page.update([notification('a', { state: 'input_needed_ignored' }), notification('b', { state: 'input_needed_ignored' })], true, { alerting: false });
  assert.equal(page.row('a').find('kind').textContent, 'Input needed · ignored');
  assert.equal(page.row('a').find('acknowledge').hidden, true);
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

test('each task shows its state and can be dismissed without affecting the others', async () => {
  const page = await ui([
    notification('a', { state: 'input_needed' }),
    notification('b', { state: 'done' }),
    notification('c', { state: 'working' }),
    notification('d', { state: 'failed' }),
  ]);
  const label = (id) => page.row(id).find('kind').textContent;
  assert.deepEqual(['a', 'b', 'c', 'd'].map(label), ['Input needed', 'Finished', 'Working', 'Failed']);
  assert.equal(page.row('c').dataset.state, 'working');
  // Busy rows have nothing to dismiss; alerting rows do.
  assert.equal(page.row('c').find('acknowledge').hidden, true);
  assert.equal(page.row('b').find('acknowledge').hidden, false);
  await page.row('b').find('acknowledge').handlers.click();
  assert.equal(JSON.stringify(page.calls.at(-1)), '["acknowledge_notification",{"id":"b"}]');
  assert.ok(!page.calls.some(([name]) => name === 'dismiss_notification' || name === 'stop_all_sound'));
  page.update([
    notification('a', { state: 'input_needed' }),
    notification('b', { state: 'done_acknowledged' }),
    notification('c', { state: 'working' }),
    notification('d', { state: 'failed' }),
  ]);
  assert.equal(label('b'), 'Finished · seen');
  assert.equal(page.row('b').find('acknowledge').hidden, true);
  assert.equal(page.row('a').find('acknowledge').hidden, false);
  assert.equal(page.row('d').find('acknowledge').hidden, false);
  // Offline disables dismissal like the other service actions.
  page.update([notification('a', { state: 'input_needed' })], false);
  assert.equal(page.row('a').find('acknowledge').disabled, true);
});

test('rows show a small mark for the reporting agent, and none when unknown', async () => {
  const page = await ui([
    notification('a', { agent: 'claude_code' }),
    notification('b', { agent: 'codex' }),
    notification('c'),
    notification('d', { agent: 'unknown' }),
  ]);
  const icon = (id) => page.row(id).find('agent-icon');
  assert.equal(icon('a').hidden, false);
  // It sits in the top line, right after the state label.
  const eyebrow = page.row('a').find('eyebrow').children;
  assert.equal(eyebrow.indexOf(icon('a')), eyebrow.indexOf(page.row('a').find('kind')) + 1);
  assert.equal(icon('a').title, 'Claude Code');
  assert.equal(icon('a').children[0].class, 'agent-mark claude_code');
  assert.equal(icon('b').title, 'Codex');
  for (const id of ['c', 'd']) {
    assert.equal(icon(id).hidden, true);
    assert.equal(icon(id).children.length, 0);
  }
  // Unchanged agents keep their node; a change swaps the mark.
  const mark = icon('a').children[0];
  page.update([notification('a', { agent: 'claude_code' })]);
  assert.equal(icon('a').children[0], mark);
  page.update([notification('a', { agent: 'codex' })]);
  assert.equal(icon('a').children[0].class, 'agent-mark codex');
  assert.equal(icon('a').children.length, 1);
});

test('the app documents its REST interface and opens the web interface', async () => {
  const page = await ui([notification()]);
  page.update([notification()], true, { alerting: true });
  await page.element('api').handlers.click();
  assert.equal(page.calls.at(-1)[0], 'open_web_interface');
  assert.match(page.row('a').find('acknowledge').title, /POST \/acknowledge\/\{id\}/);
  assert.match(page.row('a').find('clear').title, /POST \/dismiss\/\{id\}/);
});

test('each row shows a timing line above its details for running, waiting, and finished tasks', async () => {
  const ago = (ms) => new Date(Date.now() - ms).toISOString().replace('Z', '123456Z');
  const page = await ui([
    notification('w', { state: 'working', times: { tracked_since: ago(7200000), task_started_at: ago(252000), updated_at: ago(5000) } }),
    notification('s', { state: 'working', times: { task_started_at: ago(3 * 3600000 + 5 * 60000), updated_at: ago(2 * 3600000) } }),
    notification('q', { state: 'input_needed_ignored', times: { task_started_at: ago(600000), waiting_since: ago(125000), dismissed_at: ago(60000) } }),
    notification('d', { state: 'done', times: { task_started_at: ago(400000), task_finished_at: ago(130000) } }),
    notification('f', { state: 'failed_acknowledged', times: { task_finished_at: ago(2 * 86400000 + 1000) } }),
    notification('n', { state: 'done', times: { task_finished_at: ago(10000) } }),
    notification('legacy', { state: 'done' }),
  ]);
  const timing = (id) => page.row(id).find('timing');
  assert.equal(timing('w').textContent, 'Running for 4m 12s');
  assert.match(timing('w').title, /First seen: .*\nTask started: .*\nLast agent update: /);
  assert.equal(timing('s').textContent, 'Running for 3h 5m · last update 2 hours ago');
  assert.equal(timing('q').textContent, 'Waiting for 2m 5s · task running for 10m 0s');
  assert.match(timing('q').title, /Dismissed: /);
  assert.equal(timing('d').textContent, 'Finished 2 minutes ago · ran for 4m 30s');
  assert.equal(timing('f').textContent, 'Failed 2 days ago');
  assert.equal(timing('n').textContent, 'Finished less than 1 minute ago');
  assert.equal(timing('legacy').hidden, true);
  const children = page.row('d').children;
  assert.equal(children.indexOf(timing('d')) + 1, children.indexOf(page.row('d').find('details')));
  const interactions = await ui([notification('i', { state: 'working', times: { user_input_at: ago(1000), user_action_at: ago(2000), last_request_at: ago(1000) } })]);
  assert.match(interactions.row('i').find('timing').title, /Your last input in the terminal: .*\nYour last action in the app or web: .*\nLast API request: /);
});

test('timing text follows a session to its replacement row', async () => {
  const page = await ui([notification('a', { state: 'working', times: { task_started_at: new Date(Date.now() - 61000).toISOString() } })]);
  assert.equal(page.row('a').find('timing').textContent, 'Running for 1m 1s');
  const finished = new Date().toISOString();
  page.update([notification('b', { session_id: 'session-a', state: 'done', times: { task_started_at: new Date(Date.now() - 61000).toISOString(), task_finished_at: finished } })]);
  assert.equal(page.row('a'), undefined);
  assert.match(page.row('b').find('timing').textContent, /^Finished less than 1 minute ago · ran for 1m 1s$/);
});

const at = (minutes) => new Date(Date.UTC(2026, 8, 27, 12, minutes)).toISOString();
const viewTasks = () => [
  notification('ask', { state: 'input_needed', times: { updated_at: at(10), tracked_since: at(1) } }),
  notification('ignored', { state: 'input_needed_ignored', times: { updated_at: at(40), tracked_since: at(4) } }),
  notification('work', { state: 'working', times: { updated_at: at(50), tracked_since: at(2) } }),
  notification('done', { state: 'done', times: { updated_at: at(30), tracked_since: at(5) } }),
  notification('seen', { state: 'done_acknowledged', times: { updated_at: at(20), tracked_since: at(3) } }),
  notification('fail', { state: 'failed_acknowledged', times: { updated_at: at(5) } }),
];
const order = (page) => page.element('notifications').children.map((row) => row.dataset.id);

test('filters show matching tasks with counts, default to All, and never call the service', async () => {
  const page = await ui(viewTasks());
  const calls = page.calls.length;
  assert.equal(page.element('filter-all')['aria-pressed'], 'true');
  const counts = Object.fromEntries(['all', 'attention', 'working', 'asking', 'done'].map((key) => [key, page.element(`filter-${key}`).dataset.count]));
  assert.deepEqual(counts, { all: '6', attention: '2', working: '1', asking: '2', done: '3' });
  const show = (key) => { page.element(`filter-${key}`).handlers.click(); return order(page).sort(); };
  assert.deepEqual(show('attention'), ['ask', 'done']);
  assert.deepEqual(show('working'), ['work']);
  assert.deepEqual(show('asking'), ['ask', 'ignored']);
  assert.deepEqual(show('done'), ['done', 'fail', 'seen']);
  assert.equal(page.element('filter-done')['aria-pressed'], 'true');
  assert.equal(page.element('filter-all')['aria-pressed'], 'false');
  assert.equal(page.calls.length, calls);
  // Row nodes (and their open details) survive being filtered out.
  show('all');
  const work = page.row('work');
  work.find('details').open = true;
  show('done');
  assert.equal(page.row('work'), undefined);
  show('all');
  assert.equal(page.row('work'), work);
  assert.equal(work.find('details').open, true);
});

test('an empty filter explains itself and offers a way back to All', async () => {
  const page = await ui([notification('a', { state: 'done' })]);
  assert.equal(page.element('filtered-empty').hidden, true);
  page.element('filter-working').handlers.click();
  assert.equal(page.element('filtered-empty').hidden, false);
  assert.equal(page.element('filtered-empty-text').textContent, 'No agent is working.');
  page.element('filtered-reset').handlers.click();
  assert.equal(page.element('filtered-empty').hidden, true);
  assert.deepEqual(order(page), ['a']);
  page.update([]);
  assert.equal(page.element('filtered-empty').hidden, true);
  assert.equal(page.element('view-controls').hidden, true);
});

test('sorts group attention or work first and flip newest/oldest within the order', async () => {
  const page = await ui(viewTasks());
  const sort = (value) => page.element('sort-key').handlers.change({ target: { value } });
  // Default: needs you first, then newest update first.
  assert.deepEqual(order(page), ['done', 'ask', 'work', 'ignored', 'seen', 'fail']);
  page.element('sort-direction').handlers.click();
  assert.deepEqual(order(page), ['ask', 'done', 'fail', 'seen', 'ignored', 'work']);
  assert.equal(page.element('sort-direction').textContent, '↑');
  page.element('sort-direction').handlers.click();
  sort('working');
  assert.deepEqual(order(page), ['work', 'ignored', 'done', 'seen', 'ask', 'fail']);
  sort('updated');
  assert.deepEqual(order(page), ['work', 'ignored', 'done', 'seen', 'ask', 'fail']);
  // First seen; a row without the timestamp goes last in either direction.
  sort('added');
  assert.deepEqual(order(page), ['done', 'ignored', 'seen', 'work', 'ask', 'fail']);
  page.element('sort-direction').handlers.click();
  assert.deepEqual(order(page), ['ask', 'work', 'seen', 'ignored', 'done', 'fail']);
  // Sorting combines with a filter.
  page.element('filter-asking').handlers.click();
  assert.deepEqual(order(page), ['ask', 'ignored']);
});
