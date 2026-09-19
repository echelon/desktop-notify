/* The UI invokes a Rust HTTP client; conversation text is never interpreted as HTML. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const byId = (id) => document.getElementById(id);
let snapshot = { notification: null, connected: false, error: null };
let dismissing = null;

function render(next) {
  snapshot = next;
  const alert = next.notification;
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
