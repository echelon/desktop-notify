/* The UI invokes a Rust HTTP client; conversation text is never interpreted as HTML. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const byId = (id) => document.getElementById(id);
let snapshot = { notification: null, connected: false, error: null };
let dismissing = null;
let focusing = null;
let focusStatus = null;

function render(next) {
  snapshot = next;
  const alert = next.notification;
  if (focusStatus && focusStatus.id !== alert?.id) focusStatus = null;
  if (dismissing && dismissing !== alert?.id) dismissing = null;
  document.body.dataset.kind = alert?.kind || '';
  byId('alert').hidden = !alert;
  byId('actions').hidden = !alert;
  byId('empty').hidden = !!alert;
  byId('connection').textContent = next.connected ? 'Connected' : 'Service offline';
  byId('connection').classList.toggle('online', next.connected);
  byId('error').hidden = !next.error;
  byId('error').textContent = next.error || '';
  if (alert) {
    byId('kind').textContent = alert.kind === 'awaiting_user_input' ? 'Your input is needed' : 'Work completed';
    byId('title').textContent = alert.title;
    byId('message').textContent = alert.message;
  }
  byId('dismiss').disabled = !!dismissing || !next.connected;
  const origin = alert?.origin;
  const canFocus = origin && ['terminal_app', 'app_pid', 'pid', 'terminal_id', 'tty', 'window_id', 'window_title'].some((key) => origin[key]);
  byId('focus').disabled = !!focusing || !canFocus;
  byId('focus').textContent = focusing === alert?.id ? 'Focusing…' : 'Focus';
  byId('focus').title = canFocus ? 'Focus the requesting terminal without dismissing this alert' : 'No terminal information was supplied';
  byId('focus-status').hidden = !focusStatus;
  byId('focus-status').textContent = focusStatus?.message || '';
}

async function focusTerminal() {
  const id = snapshot.notification?.id;
  if (!id || focusing) return;
  focusing = id;
  focusStatus = null;
  render(snapshot);
  try {
    const result = await invoke('focus_notification', { id });
    if (snapshot.notification?.id === id) {
      focusStatus = { id, message: result.warning || `Focused the ${result.target}.` };
      await invoke('hide_window');
    }
  } catch (error) {
    if (snapshot.notification?.id === id) focusStatus = { id, message: String(error) };
  } finally {
    focusing = null;
    render(snapshot);
  }
}

async function dismiss() {
  const id = snapshot.notification?.id;
  if (!id || dismissing || !snapshot.connected) return;
  dismissing = id;
  render(snapshot);
  try { await invoke('dismiss_notification', { id }); }
  catch (error) { dismissing = null; render({ ...snapshot, error: String(error) }); }
}

byId('dismiss').addEventListener('click', dismiss);
byId('focus').addEventListener('click', focusTerminal);
byId('hide').addEventListener('click', () => invoke('hide_window'));
byId('resize').addEventListener('mousedown', (e) => { if (e.button === 0) getCurrentWindow().startResizeDragging('SouthEast'); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' || (e.metaKey && e.key === 'w')) { e.preventDefault(); invoke('hide_window'); }
  if (e.key === 'Enter' && !e.altKey && !e.metaKey && !e.ctrlKey && e.target.tagName !== 'BUTTON') { e.preventDefault(); dismiss(); }
});
(async () => {
  await listen('notification-state', ({ payload }) => render(payload));
  render(await invoke('get_snapshot'));
  await invoke('window_ready');
})().catch((error) => render({ ...snapshot, error: String(error) }));
