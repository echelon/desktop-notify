/* Conversation text is rendered only as text, never as HTML. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const byId = (id) => document.getElementById(id);
let snapshot = { notifications: [], connected: false, error: null };
const rows = new Map();
const focusing = new Set();
const pending = new Set();
const statuses = new Map();

function element(tag, className, text) {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function createRow(alert) {
  const row = element('article', 'notification');
  row.dataset.id = alert.id;
  const heading = element('div', 'row-heading');
  const info = element('div', 'row-info');
  const eyebrow = element('div', 'eyebrow');
  const kind = element('span', 'kind');
  const project = element('span', 'project');
  const session = element('span', 'session');
  eyebrow.append(kind, project, session);
  const title = element('h2', 'task-title');
  const preview = element('p', 'context-preview');
  info.append(eyebrow, title, preview);
  const controls = element('div', 'row-controls');
  const focus = element('button', 'secondary focus', 'Focus');
  focus.addEventListener('click', () => focusTerminal(alert.id));
  const clear = element('button', 'icon-button clear', '×');
  clear.title = 'Clear this entry and its sound';
  clear.setAttribute('aria-label', `Clear ${alert.title}`);
  clear.addEventListener('click', () => updateNotification(alert.id, 'dismiss_notification'));
  controls.append(focus, clear);
  heading.append(info, controls);
  const details = element('details', 'details');
  const summary = element('summary', 'summary', 'View message');
  const message = element('div', 'message');
  const context = element('dl', 'session-context');
  const fields = {};
  for (const [key, label] of Object.entries({ current_ask: 'Current task', work_arc: 'Work arc', repo_name: 'Repository', repo_description: 'About the repo', cwd: 'Directory' })) {
    const group = element('div', `context-field context-${key}`);
    const term = element('dt', 'context-label', label);
    const value = element('dd', 'context-value');
    group.append(term, value);
    context.append(group);
    fields[key] = { group, value };
  }
  details.append(summary, message, context);
  const status = element('p', 'focus-status');
  status.setAttribute('role', 'status');
  const actions = element('div', 'row-actions');
  const sound = element('span', 'sound-status');
  const silence = element('button', 'secondary silence', 'Stop sound');
  silence.addEventListener('click', () => updateNotification(alert.id, 'silence_notification'));
  actions.append(sound, silence);
  row.append(heading, details, status, actions);
  return { row, kind, project, session, title, preview, summary, context, fields, focus, clear, message, status, sound, silence };
}

const optionalText = (value) => typeof value === 'string' ? value.trim() : '';
const directoryName = (path) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;

function renderContext(refs, alert) {
  const context = alert.context || {};
  const cwd = optionalText(context.cwd);
  const repo = optionalText(context.repo_name);
  const project = repo || (cwd && directoryName(cwd));
  refs.project.hidden = !project;
  refs.project.textContent = project;
  refs.project.title = [repo, cwd].filter(Boolean).join('\n');
  if (project && alert.session_id) refs.session.textContent = alert.session_id.slice(-8);
  // The hooks prefix their alert title with the directory; avoid repeating it.
  const prefix = cwd && `${directoryName(cwd)}: `;
  refs.title.textContent = project && prefix && alert.title.startsWith(prefix) ? alert.title.slice(prefix.length) : alert.title;
  const ask = optionalText(context.current_ask);
  const arc = optionalText(context.work_arc);
  const preview = ask || arc;
  refs.preview.hidden = !preview || preview === refs.title.textContent;
  refs.preview.textContent = preview ? `${ask ? 'Task' : 'Work'} · ${preview}` : '';
  refs.preview.title = preview;
  let hasContext = false;
  for (const [key, { group, value }] of Object.entries(refs.fields)) {
    const text = optionalText(context[key]);
    group.hidden = !text;
    value.textContent = text;
    hasContext ||= !!text;
  }
  refs.context.hidden = !hasContext;
  refs.summary.textContent = hasContext ? 'Details' : 'View message';
}

function render(next) {
  snapshot = next;
  const alerts = next.notifications;
  const ids = new Set(alerts.map((alert) => alert.id));
  for (const [id, refs] of rows) {
    if (!ids.has(id)) {
      refs.row.remove();
      rows.delete(id);
      statuses.delete(id);
    }
  }
  byId('empty').hidden = alerts.length > 0;
  byId('notifications').hidden = alerts.length === 0;
  byId('count').textContent = alerts.length ? String(alerts.length) : '';
  byId('connection').textContent = next.connected ? 'Connected' : 'Service offline';
  byId('connection').classList.toggle('online', next.connected);
  byId('error').hidden = !next.error;
  byId('error').textContent = next.error || '';
  alerts.forEach((alert, index) => {
    if (!rows.has(alert.id)) rows.set(alert.id, createRow(alert));
    const refs = rows.get(alert.id);
    refs.row.dataset.kind = alert.kind;
    refs.kind.textContent = alert.kind === 'awaiting_user_input' ? 'Input needed' : 'Finished';
    refs.session.textContent = alert.session_id ? `Session ${alert.session_id.slice(-8)}` : 'Unassigned';
    refs.session.title = alert.session_id || 'No session ID supplied';
    refs.title.textContent = alert.title;
    refs.title.title = alert.title;
    refs.message.textContent = alert.message;
    renderContext(refs, alert);
    const origin = alert.origin;
    const canFocus = origin && ['terminal_app', 'app_pid', 'pid', 'terminal_id', 'tty', 'window_id', 'window_title'].some((key) => origin[key]);
    refs.focus.disabled = focusing.has(alert.id) || !canFocus;
    refs.focus.textContent = focusing.has(alert.id) ? 'Focusing…' : 'Focus';
    refs.focus.title = canFocus ? 'Focus the requesting terminal; keep this entry visible' : 'No terminal information was supplied';
    refs.clear.disabled = pending.has(alert.id) || !next.connected;
    refs.silence.disabled = pending.has(alert.id) || !next.connected || alert.silenced;
    refs.silence.textContent = alert.silenced ? 'Silenced' : 'Stop sound';
    refs.sound.textContent = alert.silenced ? 'Sound stopped · status retained' : 'Sound on';
    refs.status.hidden = !statuses.has(alert.id);
    refs.status.textContent = statuses.get(alert.id) || '';
    const list = byId('notifications');
    // Preserve open details, selection and keyboard focus on unchanged updates.
    if (list.children[index] !== refs.row) list.insertBefore(refs.row, list.children[index] || null);
  });
}

async function focusTerminal(id) {
  if (!rows.has(id) || focusing.has(id)) return;
  focusing.add(id);
  statuses.delete(id);
  render(snapshot);
  try {
    const result = await invoke('focus_notification', { id });
    if (rows.has(id)) statuses.set(id, result.warning || `Focused the ${result.target}.`);
  } catch (error) {
    if (rows.has(id)) statuses.set(id, String(error));
  } finally {
    focusing.delete(id);
    render(snapshot);
  }
}

async function updateNotification(id, command) {
  if (!rows.has(id) || pending.has(id) || !snapshot.connected) return;
  pending.add(id);
  statuses.delete(id);
  render(snapshot);
  try { await invoke(command, { id }); }
  catch (error) { if (rows.has(id)) statuses.set(id, String(error)); }
  finally {
    pending.delete(id);
    render(snapshot);
  }
}

byId('hide').addEventListener('click', () => invoke('hide_window'));
byId('resize').addEventListener('mousedown', (e) => { if (e.button === 0) getCurrentWindow().startResizeDragging('SouthEast'); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' || (e.metaKey && e.key === 'w')) { e.preventDefault(); invoke('hide_window'); }
});
(async () => {
  await listen('notification-state', ({ payload }) => render(payload));
  render(await invoke('get_snapshot'));
  await invoke('window_ready');
})().catch((error) => render({ ...snapshot, error: String(error) }));
