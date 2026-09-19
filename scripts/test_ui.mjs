import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../ui/app.js', import.meta.url), 'utf8');
const notification = (id = 'a', origin = { terminal_app: 'ghostty' }) => ({ id, title: 'Task', message: 'Question?', kind: 'awaiting_user_input', origin });

async function ui(alert, focus = async () => ({ target: 'application' })) {
  const elements = new Map();
  const calls = [];
  let update;
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, { classList: { toggle() {} }, handlers: {}, addEventListener(event, handler) { this.handlers[event] = handler; } });
    return elements.get(id);
  };
  const context = {
    document: { body: { dataset: {} }, getElementById: element, addEventListener() {} },
    window: { __TAURI__: {
      core: { async invoke(command, args) {
        calls.push([command, args]);
        if (command === 'get_snapshot') return { notification: alert, connected: true, error: null };
        if (command === 'focus_notification') return focus(args);
      } },
      event: { async listen(_event, handler) { update = handler; } },
      window: { getCurrentWindow() { return {}; } },
    } },
  };
  vm.runInNewContext(source, context);
  await new Promise(setImmediate);
  return { element, calls, update: (alert) => update({ payload: { notification: alert, connected: true, error: null } }) };
}

test('legacy alerts have a disabled Focus button', async () => {
  const page = await ui(notification('a', null));
  assert.equal(page.element('focus').disabled, true);
  assert.match(page.element('focus').title, /No terminal information/);
});

test('Focus activates the originating task and hides without dismissing', async () => {
  const page = await ui(notification());
  assert.equal(page.element('focus').disabled, false);
  await page.element('focus').handlers.click();
  assert.equal(page.calls.find(([name]) => name === 'focus_notification')[1].id, 'a');
  assert.ok(page.calls.some(([name]) => name === 'hide_window'));
  assert.ok(!page.calls.some(([name]) => name === 'dismiss_notification'));
});

test('a focus error remains visible across unchanged server updates', async () => {
  const page = await ui(notification(), async () => { throw new Error('App exited'); });
  await page.element('focus').handlers.click();
  page.update(notification());
  assert.equal(page.element('focus-status').hidden, false);
  assert.match(page.element('focus-status').textContent, /App exited/);
  assert.ok(!page.calls.some(([name]) => name === 'hide_window'));
});

test('completion of an older Focus request cannot hide a replacement alert', async () => {
  let resolve;
  const page = await ui(notification(), () => new Promise((r) => { resolve = r; }));
  const pending = page.element('focus').handlers.click();
  assert.equal(page.element('focus').disabled, true);
  page.update(notification('b'));
  resolve({ target: 'application' });
  await pending;
  assert.ok(!page.calls.some(([name]) => name === 'hide_window'));
  assert.equal(page.element('focus-status').hidden, true);
  assert.equal(page.element('focus').disabled, false);
});
