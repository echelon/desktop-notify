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

async function ui(alerts, invoke = async () => ({ target: 'application' })) {
  const elements = new Map();
  const calls = [];
  let update;
  const element = (id) => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
  const context = {
    document: { createElement: (tag) => new Element(tag), getElementById: element, addEventListener() {} },
    window: { __TAURI__: {
      core: { async invoke(command, args) {
        calls.push([command, args]);
        if (command === 'get_snapshot') return { notifications: alerts, connected: true, error: null };
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
    update: (notifications, connected = true) => update({ payload: { notifications, connected, error: null } }),
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

test('Focus leaves all rows available for independent sound stopping and clearing', async () => {
  const page = await ui([notification(), notification('b')]);
  await page.row('b').find('focus').handlers.click();
  assert.equal(page.calls.find(([name]) => name === 'focus_notification')[1].id, 'b');
  assert.ok(!page.calls.some(([name]) => name === 'hide_window' || name === 'dismiss_notification'));
  assert.equal(page.row('b').find('silence').disabled, false);
  await page.row('b').find('silence').handlers.click();
  assert.equal(page.calls.find(([name]) => name === 'silence_notification')[1].id, 'b');
  page.update([notification(), notification('b', { silenced: true })]);
  assert.equal(page.row('b').find('silence').disabled, true);
  assert.equal(page.row('a').find('silence').disabled, false);
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
  assert.equal(page.row('b').find('silence').disabled, true);
  assert.equal(page.row('a').find('focus').disabled, false);
  page.update([]);
  assert.equal(page.element('empty').hidden, false);
});
