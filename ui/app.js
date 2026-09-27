/* Conversation text is rendered only as text, never as HTML. */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const byId = (id) => document.getElementById(id);
let snapshot = { notifications: [], sound: {}, connected: false, error: null };
const rows = new Map();
const focusing = new Set();
const pending = new Set();
const statuses = new Map();
let soundPending = false;
let soundError = '';

const STATES = {
  working: { label: 'Working', alerting: false },
  input_needed: { label: 'Input needed', alerting: true },
  input_needed_ignored: { label: 'Input needed · ignored', alerting: false },
  done: { label: 'Finished', alerting: true },
  done_acknowledged: { label: 'Finished · seen', alerting: false },
  failed: { label: 'Failed', alerting: true },
  failed_acknowledged: { label: 'Failed · seen', alerting: false },
};
const taskState = (alert) => (STATES[alert.state] ? alert.state : 'done');

// Small agent marks use fixed local artwork, never row-provided markup or URLs.
const SVG_NS = 'http://www.w3.org/2000/svg';
// Claude Code's mascot, from its terminal banner. Each block character is a
// 2×2 grid of quadrants (upper-left, upper-right, lower-left, lower-right);
// terminal cells are twice as tall as wide, so each quadrant is 1×2 units.
const QUADRANTS = { '▐': '0101', '▛': '1110', '█': '1111', '▜': '1101', '▌': '1010', '▝': '0100', '▘': '1000' };
const CLAUDE_MASCOT = [' ▐▛███▜▌', '▝▜█████▛▘', '  ▘▘ ▝▝'].flatMap((line, row) =>
  [...line].flatMap((char, column) => [...(QUADRANTS[char] || '0000')].flatMap((filled, quadrant) =>
    filled === '1' ? [`M${column * 2 + (quadrant % 2)} ${row * 4 + Math.floor(quadrant / 2) * 2}h1v2h-1z`] : []))).join('');
const AGENTS = {
  claude_code: { label: 'Claude Code', path: CLAUDE_MASCOT, viewBox: '0 0 18 12' },
  codex: { label: 'Codex', src: 'assets/codex.svg' },
};

function agentMark(agent) {
  if (AGENTS[agent].src) {
    const image = document.createElement('img');
    image.setAttribute('src', AGENTS[agent].src);
    image.setAttribute('alt', '');
    image.setAttribute('aria-hidden', 'true');
    image.setAttribute('class', `agent-mark ${agent}`);
    return image;
  }
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', AGENTS[agent].viewBox);
  // Wide marks start at the left and center on the text line.
  svg.setAttribute('preserveAspectRatio', 'xMinYMid meet');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', `agent-mark ${agent}`);
  const path = document.createElementNS(SVG_NS, 'path');
  path.setAttribute('d', AGENTS[agent].path);
  svg.append(path);
  return svg;
}

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
  // The reporting agent's mark follows the state label on the same line.
  const agent = element('span', 'agent-icon');
  agent.setAttribute('role', 'img');
  eyebrow.append(kind, agent, project, session);
  const title = element('h2', 'task-title');
  const preview = element('p', 'context-preview');
  info.append(eyebrow, title, preview);
  const controls = element('div', 'row-controls');
  const focus = element('button', 'secondary focus', 'Focus');
  focus.addEventListener('click', () => focusTerminal(alert.id));
  const acknowledge = element('button', 'secondary acknowledge', 'Dismiss');
  acknowledge.title = 'Stop alerting for this task; keep it listed. Other tasks keep alerting.\nPOST /acknowledge/{id}';
  acknowledge.setAttribute('aria-label', `Dismiss ${alert.title}`);
  acknowledge.addEventListener('click', () => updateNotification(alert.id, 'acknowledge_notification'));
  const clear = element('button', 'icon-button clear', '×');
  clear.title = 'Clear this entry and its sound\nPOST /dismiss/{id}';
  clear.setAttribute('aria-label', `Clear ${alert.title}`);
  clear.addEventListener('click', () => updateNotification(alert.id, 'dismiss_notification'));
  controls.append(focus, acknowledge, clear);
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
  row.append(heading, details, status);
  return { row, agent, kind, project, session, title, preview, summary, context, fields, focus, acknowledge, clear, message, status };
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
  if (next.service) byId('api').title = `Web interface and REST API reference\n${next.service}/`;
  byId('connection').classList.toggle('online', next.connected);
  byId('error').hidden = !next.error;
  byId('error').textContent = next.error || '';
  alerts.forEach((alert, index) => {
    if (!rows.has(alert.id)) rows.set(alert.id, createRow(alert));
    const refs = rows.get(alert.id);
    const state = taskState(alert);
    refs.row.dataset.state = state;
    refs.row.classList.toggle('quiet', !STATES[state].alerting);
    refs.kind.textContent = STATES[state].label;
    const known = AGENTS[alert.agent] ? alert.agent : null;
    refs.agent.hidden = !known;
    if (refs.agent.dataset.agent !== (known || '')) {
      refs.agent.dataset.agent = known || '';
      refs.agent.replaceChildren(...(known ? [agentMark(known)] : []));
      refs.agent.title = known ? AGENTS[known].label : '';
      refs.agent.setAttribute('aria-label', known ? `Reported by ${AGENTS[known].label}` : '');
    }
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
    // Only alerting rows can be dismissed; quiet rows are already acknowledged.
    refs.acknowledge.hidden = !STATES[state].alerting;
    refs.acknowledge.disabled = pending.has(alert.id) || !next.connected;
    refs.status.hidden = !statuses.has(alert.id);
    refs.status.textContent = statuses.get(alert.id) || '';
    const list = byId('notifications');
    // Preserve open details, selection and keyboard focus on unchanged updates.
    if (list.children[index] !== refs.row) list.insertBefore(refs.row, list.children[index] || null);
  });
  renderSound();
}

function formatRemaining(ms) {
  const seconds = Math.ceil(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = String(Math.floor(seconds / 60) % 60);
  return `${hours ? `${hours}:${minutes.padStart(2, '0')}` : minutes}:${String(seconds % 60).padStart(2, '0')}`;
}

/* Sound is global: every alerting row feeds one shared loop. The service owns
   the snooze deadline; this only compares it with the clock for the countdown. */
function renderSound() {
  const sound = snapshot.sound || {};
  const remaining = sound.snoozed_until ? Date.parse(sound.snoozed_until) - Date.now() : 0;
  const snoozed = remaining > 0;
  byId('sound-controls').hidden = snapshot.notifications.length === 0 && !snoozed;
  byId('sound-controls').classList.toggle('snoozed', snoozed);
  const unavailable = soundPending || !snapshot.connected;
  byId('silence-all').disabled = unavailable || (!sound.alerting && !snoozed);
  byId('snooze-1').disabled = unavailable;
  byId('snooze-5').disabled = unavailable;
  let text = sound.alerting ? 'Sound on' : 'All sounds stopped';
  if (snoozed) text = `Snoozed · resumes in ${formatRemaining(remaining)}`;
  byId('sound-status').textContent = soundError || text;
}

async function updateSound(command, args) {
  if (soundPending || !snapshot.connected) return;
  soundPending = true;
  soundError = '';
  renderSound();
  try { await invoke(command, args); }
  catch (error) { soundError = String(error); }
  finally {
    soundPending = false;
    renderSound();
  }
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

byId('silence-all').addEventListener('click', () => updateSound('stop_all_sound'));
byId('snooze-1').addEventListener('click', () => updateSound('snooze_sound', { seconds: 60 }));
byId('snooze-5').addEventListener('click', () => updateSound('snooze_sound', { seconds: 300 }));
byId('api').addEventListener('click', () => invoke('open_web_interface').catch((error) => { soundError = String(error); renderSound(); }));
// Refreshes only the countdown text; resuming is decided by the service.
setInterval(renderSound, 1000);
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
