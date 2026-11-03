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
  claude_code: { label: 'Claude Code', path: CLAUDE_MASCOT, viewBox: '1 0 16 10' },
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

const FULL_VOLUME = 100;
const volumeOf = (source) => ({ volume: source?.volume ?? FULL_VOLUME, muted: !!source?.muted });
// How long a control keeps showing its own value after the last send, so a poll
// that left before that send landed cannot flick the slider back.
const VOLUME_SETTLE_MS = 600;

function speakerIcon() {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  for (const [className, d] of [['body', 'M4 9.5v5h3.5L12 18.5v-13L7.5 9.5z'], ['waves', 'M15.5 9.5a3.5 3.5 0 0 1 0 5M18 7a7 7 0 0 1 0 10'], ['slash', 'M16 9.5l5 5M21 9.5l-5 5']]) {
    const path = document.createElementNS(SVG_NS, 'path');
    path.setAttribute('class', className);
    path.setAttribute('d', d);
    svg.append(path);
  }
  return svg;
}

/* One mute button and slider. `send` posts a change. While the user is adjusting,
   or a change is settling, the control shows its own value instead of the poll's;
   this is transient UI state, and the service still owns the volume. At most one
   request is in flight, and only the newest value is sent next, so a fast drag
   never queues a backlog. Each control belongs to one row ID, so a delayed send
   for a replaced row reaches only its stale ID, which the service ignores. */
function createVolumeControl(send, reportError) {
  const wrap = element('div', 'volume-control');
  const mute = element('button', 'volume-mute');
  mute.setAttribute('type', 'button');
  mute.append(speakerIcon());
  const slider = element('input', 'volume-slider');
  slider.type = 'range';
  slider.min = '0';
  slider.max = String(FULL_VOLUME);
  slider.step = '1';
  const value = element('span', 'volume-value');
  wrap.append(mute, slider, value);
  const control = { wrap, mute, slider, value, held: false, settleUntil: 0, local: { volume: FULL_VOLUME, muted: false }, queued: null, sending: false };

  async function flush() {
    if (control.sending) return;
    control.sending = true;
    while (control.queued) {
      const change = control.queued;
      control.queued = null;
      try { await send(change); }
      catch (error) { reportError(String(error)); }
    }
    control.sending = false;
    control.settleUntil = Date.now() + VOLUME_SETTLE_MS;
  }
  function change(update) {
    Object.assign(control.local, update);
    control.queued = { ...control.queued, ...update };
    control.settleUntil = Date.now() + VOLUME_SETTLE_MS;
    paintVolume(control);
    return flush();
  }
  control.change = change;
  mute.addEventListener('click', () => change({ muted: !control.local.muted }));
  slider.addEventListener('input', () => change({ volume: Number(slider.value) }));
  slider.addEventListener('pointerdown', () => { control.held = true; });
  for (const event of ['pointerup', 'pointercancel', 'change']) slider.addEventListener(event, () => { control.held = false; });
  return control;
}

function paintVolume(control) {
  const { volume, muted } = control.local;
  const text = String(volume);
  // Never move a thumb the user is holding, and only touch the DOM on change.
  if (!control.held && control.slider.value !== text) control.slider.value = text;
  control.value.textContent = muted ? 'Muted' : `${volume}%`;
  control.mute.setAttribute('aria-pressed', String(muted));
  control.wrap.classList.toggle('is-muted', muted);
}

function renderVolume(control, source, { enabled, label, muteTitle, sliderTitle }) {
  const adjusting = control.held || control.sending || Date.now() < control.settleUntil;
  if (!adjusting) control.local = volumeOf(source);
  paintVolume(control);
  control.mute.disabled = !enabled;
  control.slider.disabled = !enabled;
  control.mute.setAttribute('aria-label', `${control.local.muted ? 'Unmute' : 'Mute'} ${label}`);
  control.mute.title = muteTitle;
  control.slider.setAttribute('aria-label', `Volume for ${label}`);
  control.slider.title = sliderTitle(control.local);
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
  const timing = element('p', 'timing');
  const volume = createVolumeControl(
    (change) => invoke('set_notification_volume', { id: alert.id, ...change }),
    (error) => { if (rows.has(alert.id)) { statuses.set(alert.id, error); render(snapshot); } },
  );
  const meta = element('div', 'row-meta');
  meta.append(timing, volume.wrap);
  // Timing and this row's volume sit between the task line and the details.
  row.append(heading, meta, details, status);
  return { row, volume, agent, kind, project, session, title, preview, summary, context, fields, focus, acknowledge, clear, message, status, timing };
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

/* Timestamps come from the service (RFC 3339 UTC). They are compared with the
   local clock once a second so the timing line stays current between polls. */
const MINUTE = 60000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
// WebKit's Date.parse is unreliable past millisecond precision; chrono sends up to nanoseconds.
const parseTime = (value) => typeof value === 'string' ? Date.parse(value.replace(/(\.\d{3})\d+/, '$1')) : NaN;
const plural = (count, unit) => `${count} ${unit}${count === 1 ? '' : 's'}`;

function timeAgo(ms) {
  if (ms < MINUTE) return 'less than 1 minute ago';
  if (ms < HOUR) return `${plural(Math.floor(ms / MINUTE), 'minute')} ago`;
  if (ms < DAY) return `${plural(Math.floor(ms / HOUR), 'hour')} ago`;
  return `${plural(Math.floor(ms / DAY), 'day')} ago`;
}

function formatDuration(ms) {
  const seconds = Math.floor(Math.max(0, ms) / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  if (seconds < 60) return `${seconds}s`;
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

const TIME_LABELS = {
  tracked_since: 'First seen', task_started_at: 'Task started', waiting_since: 'Waiting since', task_finished_at: 'Task ended',
  dismissed_at: 'Dismissed', updated_at: 'Last agent update', user_input_at: 'Your last input in the terminal',
  user_action_at: 'Your last action in the app or web', last_request_at: 'Last API request',
};

/* One line per row: how long the task has run (or ran), how long it has
   waited, and how long ago it ended. Unknown moments are left out. */
function timingText(alert, now) {
  const times = alert.times || {};
  const at = Object.fromEntries(Object.keys(TIME_LABELS).map((key) => [key, parseTime(times[key])]));
  const since = (time) => Math.max(0, now - time);
  const parts = [];
  const state = taskState(alert);
  if (state === 'working') {
    parts.push(at.task_started_at ? `Running for ${formatDuration(since(at.task_started_at))}` : 'Running');
    // Quiet agents are worth noticing: the hooks report every finished tool.
    if (since(at.updated_at) >= MINUTE) parts.push(`last update ${timeAgo(since(at.updated_at))}`);
  } else if (state.startsWith('input_needed')) {
    if (at.waiting_since) parts.push(`Waiting for ${formatDuration(since(at.waiting_since))}`);
    if (at.task_started_at) parts.push(`task running for ${formatDuration(since(at.task_started_at))}`);
  } else if (at.task_finished_at) {
    parts.push(`${state.startsWith('failed') ? 'Failed' : 'Finished'} ${timeAgo(since(at.task_finished_at))}`);
    if (at.task_started_at) parts.push(`ran for ${formatDuration(at.task_finished_at - at.task_started_at)}`);
  }
  if (!parts.length && at.updated_at) parts.push(`Updated ${timeAgo(since(at.updated_at))}`);
  if (parts.length) parts[0] = parts[0][0].toUpperCase() + parts[0].slice(1);
  const detail = Object.entries(TIME_LABELS).filter(([key]) => at[key])
    .map(([key, label]) => `${label}: ${new Date(at[key]).toLocaleString()}`).join('\n');
  return { text: parts.join(' · '), detail };
}

function renderTiming(refs, alert, now = Date.now()) {
  const { text, detail } = timingText(alert, now);
  // Only touch the DOM on change so the once-a-second refresh keeps selections.
  if (refs.timing.textContent !== text) refs.timing.textContent = text;
  if (refs.timing.title !== detail) refs.timing.title = detail;
  refs.timing.hidden = !text;
}

function renderTimings() {
  const now = Date.now();
  for (const alert of snapshot.notifications) {
    if (rows.has(alert.id)) renderTiming(rows.get(alert.id), alert, now);
  }
}

/* Filter and sort are presentation only: they never change a task or reach the
   service, reset to All on each launch, and leave the tray count untouched.
   The direction flips newest/oldest within the chosen order. */
const FILTERS = {
  all: () => true,
  attention: (state) => STATES[state].alerting,
  asking: (state) => state.startsWith('input_needed'),
  working: (state) => state === 'working',
  done: (state) => state.startsWith('done') || state.startsWith('failed'),
};
const SORTS = {
  attention: { rank: (state) => (STATES[state].alerting ? 0 : 1), time: 'updated_at' },
  working: { rank: (state) => (state === 'working' ? 0 : 1), time: 'updated_at' },
  updated: { rank: () => 0, time: 'updated_at' },
  added: { rank: () => 0, time: 'tracked_since' },
};
const FILTER_EMPTY = { all: 'No tasks.', attention: 'Nothing needs you.', working: 'No agent is working.', asking: 'No agent is asking.', done: 'No finished tasks.' };
const view = { filter: 'all', sort: 'attention', newestFirst: true };

function visibleTasks(alerts) {
  const sort = SORTS[view.sort];
  const direction = view.newestFirst ? 1 : -1;
  // Rows without the timestamp (from older services) go last. The service lists
  // rows newest update first, and that order breaks ties.
  return alerts.map((alert, index) => ({ alert, index, state: taskState(alert), time: parseTime(alert.times?.[sort.time]) }))
    .filter(({ state }) => FILTERS[view.filter](state))
    .sort((a, b) => sort.rank(a.state) - sort.rank(b.state)
      || Number.isNaN(a.time) - Number.isNaN(b.time)
      || direction * ((b.time - a.time) || (a.index - b.index)))
    .map(({ alert }) => alert);
}

function renderView(alerts) {
  byId('view-controls').hidden = alerts.length === 0;
  for (const key of Object.keys(FILTERS)) {
    const chip = byId(`filter-${key}`);
    const count = key === 'all' ? alerts.length : alerts.filter((alert) => FILTERS[key](taskState(alert))).length;
    chip.dataset.count = count ? String(count) : '';
    chip.setAttribute('aria-pressed', String(view.filter === key));
    chip.classList.toggle('has-items', count > 0);
  }
  byId('sort-key').value = view.sort;
  byId('sort-direction').textContent = view.newestFirst ? '↓' : '↑';
  byId('sort-direction').title = view.newestFirst ? 'Newest first (click for oldest first)' : 'Oldest first (click for newest first)';
  byId('sort-direction').setAttribute('aria-label', view.newestFirst ? 'Newest first' : 'Oldest first');
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
  alerts.forEach((alert) => {
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
    refs.focus.title = canFocus ? 'Focus the requesting terminal; keep this entry visible\nPOST /focused/{id} (records the time only)' : 'No terminal information was supplied';
    refs.clear.disabled = pending.has(alert.id) || !next.connected;
    // Only alerting rows can be dismissed; quiet rows are already acknowledged.
    refs.acknowledge.hidden = !STATES[state].alerting;
    refs.acknowledge.disabled = pending.has(alert.id) || !next.connected;
    const global = volumeOf(next.sound);
    renderVolume(refs.volume, alert, {
      enabled: next.connected,
      label: alert.title,
      muteTitle: 'Mute this task only; other tasks keep sounding\nPOST /volume/{id} {"muted": true}',
      sliderTitle: ({ volume, muted }) => {
        const plays = muted || global.muted ? 0 : Math.round(volume * global.volume / FULL_VOLUME);
        return `This task ${volume}% × all tasks ${global.volume}% = plays at ${plays}%\nPOST /volume/{id} {"volume": 0–100}`;
      },
    });
    refs.status.hidden = !statuses.has(alert.id);
    refs.status.textContent = statuses.get(alert.id) || '';
    renderTiming(refs, alert);
  });
  // Filtered-out rows leave the list but keep their nodes, so open details and
  // per-row status survive switching back.
  const visible = visibleTasks(alerts);
  const shown = new Set(visible.map((alert) => alert.id));
  for (const [id, refs] of rows) if (!shown.has(id)) refs.row.remove();
  const list = byId('notifications');
  visible.forEach((alert, index) => {
    const { row } = rows.get(alert.id);
    // Preserve open details, selection and keyboard focus on unchanged updates.
    if (list.children[index] !== row) list.insertBefore(row, list.children[index] || null);
  });
  renderView(alerts);
  byId('filtered-empty').hidden = alerts.length === 0 || visible.length > 0;
  byId('filtered-empty-text').textContent = FILTER_EMPTY[view.filter];
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
  byId('clear-all').disabled = unavailable || snapshot.notifications.length === 0;
  renderVolume(masterVolume, sound, {
    enabled: snapshot.connected,
    label: 'all tasks',
    muteTitle: 'Mute every task; alerts stay listed\nPOST /sound/volume {"muted": true}',
    sliderTitle: ({ volume }) => `Volume for all tasks: ${volume}%, multiplied by each task's own volume\nPOST /sound/volume {"volume": 0–100}`,
  });
  // Muted rows are skipped, so sound only stops if nothing audible is left.
  const master = volumeOf(sound);
  const audible = !master.muted && master.volume > 0 && snapshot.notifications.some((alert) => {
    const row = volumeOf(alert);
    return STATES[taskState(alert)].alerting && !row.muted && row.volume > 0;
  });
  let text = sound.alerting ? (audible ? 'Sound on' : 'Muted') : 'All sounds stopped';
  if (snoozed) text = `Snoozed · resumes in ${formatRemaining(remaining)}`;
  byId('sound-status').textContent = soundError || text;
}

/* Global volume. Created up front: the bar stays visible with no tasks. */
const masterVolume = createVolumeControl(
  (change) => invoke('set_sound_volume', change),
  (error) => { soundError = error; renderSound(); },
);
byId('master-volume').append(masterVolume.wrap);

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
byId('clear-all').addEventListener('click', () => updateSound('clear_all_notifications'));
byId('snooze-1').addEventListener('click', () => updateSound('snooze_sound', { seconds: 60 }));
byId('snooze-5').addEventListener('click', () => updateSound('snooze_sound', { seconds: 300 }));
byId('api').addEventListener('click', () => invoke('open_web_interface').catch((error) => { soundError = String(error); renderSound(); }));
// Refreshes only the countdown and timing text; resuming is decided by the service.
setInterval(() => { renderSound(); renderTimings(); }, 1000);
byId('hide').addEventListener('click', () => invoke('hide_window'));
const setView = (change) => { Object.assign(view, change); render(snapshot); };
for (const key of Object.keys(FILTERS)) byId(`filter-${key}`).addEventListener('click', () => setView({ filter: key }));
byId('filtered-reset').addEventListener('click', () => setView({ filter: 'all' }));
byId('sort-key').addEventListener('change', (event) => setView({ sort: SORTS[event.target.value] ? event.target.value : 'attention' }));
byId('sort-direction').addEventListener('click', () => setView({ newestFirst: !view.newestFirst }));
byId('resize').addEventListener('mousedown', (e) => { if (e.button === 0) getCurrentWindow().startResizeDragging('SouthEast'); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' || (e.metaKey && e.key === 'w')) { e.preventDefault(); invoke('hide_window'); }
});
(async () => {
  await listen('notification-state', ({ payload }) => render(payload));
  render(await invoke('get_snapshot'));
  await invoke('window_ready');
})().catch((error) => render({ ...snapshot, error: String(error) }));
