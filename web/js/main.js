// 入口：装配工作区侧栏 / 时间线 / 输入区 / 右栏停靠，接通 SSE 双流与任务生命周期。
// 工作区切换 = setWorkspacePrefix + pushState + 数据重载 + SSE 重挂 + 设备面板刷新。

import { api, apiPost, apiPatch, openSse, array, setWorkspacePrefix } from './api.js';
import { state, on, draftKey, messagesOf, viewKeyOf, tierNames } from './store.js';
import { h, cn, button, badge, spinner, openDialog, toast, bindTooltip } from './ui.js';
import { createWorkbenchSidebar } from './sidebar.js';
import { createBrowserPane } from './panels/browser.js';
import { createTerminalPane } from './panels/terminal.js';
import { createFilesPanel } from './panels/files.js';
import { createGitPanel } from './panels/git.js';
import { createModelsPanel } from './panels/models.js';
import { createAgentsPanel } from './panels/agents.js';
import { createTasksPanel } from './panels/tasks.js';
import { createDock } from './dock.js';
import { workspaceIdFromLocation } from './workspaces.js';
import { createTimeline } from './timeline/render.js';
import { createLiveToolState } from './timeline/projection.js';
import { createComposer } from './composer.js';
import { createTrajectoryPanel } from './trajectory/render.js';
import { requestSummary } from './model-select.js';
import { mountGridBeam } from './cult/gridbeam.js';
import { rollingNumber, setHalo } from './motion.js';
import { apiPath } from './api.js';

const byId = (id) => document.getElementById(id);

const liveTools = createLiveToolState();
const timeline = createTimeline(byId('timeline'));
const gridbeam = mountGridBeam(byId('gridbeam'));
// 轮次计数滚柱：随任务状态标签复用（元素持久 → 数字滚动连续）。
const taskTurnRoll = h('span', { class: 'roll-num mono' });
const trajectory = createTrajectoryPanel(byId('panel-trajectory'), {
  getExportUrl: () => state.traceId ? apiPath(`/api/traces/${encodeURIComponent(state.traceId)}/export`) : null,
});
let sidebar;
let composer;
let dock;
let files;
let git;
let terminal;
let browser;
let modelsPanel;
let agentsPanel;
let pollTimer = 0;
let progressTimer = 0;
let streamSources = [];
let currentWorkspaceId = workspaceIdFromLocation();
// 每个工作区记住上次打开的会话，切回时恢复。
const wsSessionMemory = new Map([['default', null]]);

/* ---------- 主题 ---------- */
function resolveTheme(theme) {
  if (theme !== 'system') return theme;
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}
function applyTheme(theme) {
  state.ui.theme = theme;
  document.body.dataset.theme = resolveTheme(theme);
}
window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
  if (state.ui.theme === 'system') applyTheme('system');
});

function setConnection(online) {
  state.apiOnline = online;
  const dot = byId('connection-dot');
  dot.classList.toggle('online', online);
  dot.classList.toggle('offline', !online);
  byId('connection-label').textContent = online ? '运行时已连接' : '运行时不可达';
}

/* ---------- 会话与时间线 ---------- */
function conversationTitle(session) {
  if (!state.currentSessionId) return '从一个任务开始';
  const found = state.sessions.find((s) => s && s.id === state.currentSessionId);
  return String(found?.title || state.currentSessionId);
}

function syncTimeline(live = null) {
  const key = state.currentSessionId ? viewKeyOf(state.currentSessionId) : state.draftKeyValue;
  const count = timeline.sync(messagesOf(key || draftKey()), live);
  // 空态（草稿或无消息会话）显示唯一 canvas 背景，有内容即让位。
  gridbeam.setActive(count === 0 && !live);
}

function updateHeader() {
  byId('conversation-title').textContent = conversationTitle();
  const archived = state.sessions.find((s) => s?.id === state.currentSessionId)?.archived;
  byId('conversation-sub').textContent = state.currentSessionId
    ? `会话 ${state.currentSessionId.slice(0, 8)} · ${archived ? '已归档 · 发送消息将自动取消归档' : '继续对话将追加到本会话'}`
    : '草稿 · 发送首条消息后落库';
  byId('fork-session').disabled = !state.currentSessionId || Boolean(state.task);
}

async function loadSession(id, { force = false } = {}) {
  const request = ++state.requestCounter;
  state.currentSessionId = id;
  wsSessionMemory.set(currentWorkspaceId, id);
  const key = viewKeyOf(id);
  if (key && !state.transcripts.has(key)) state.transcripts.set(key, []);
  sidebar.setCurrent(id);
  timeline.reset();
  syncTimeline();
  updateHeader();
  if (!id) return;
  try {
    const detail = await api(`/api/sessions/${encodeURIComponent(id)}`);
    if (request !== state.requestCounter) return;
    state.transcripts.set(key, array(detail?.messages, 'messages'));
    const latestTrace = await latestSessionTrace(id);
    state.traceId = latestTrace?.trace_id || '';
    trajectory.setEvents(collectTraceEvents(latestTrace?.trace_id || ''));
    syncTimeline();
  } catch (error) {
    if (request === state.requestCounter) toast(`无法读取会话：${error.message}`, 'error');
  }
}

async function latestSessionTrace(sessionId) {
  try {
    const page = await api(`/api/events/page?session_id=${encodeURIComponent(sessionId)}&limit=500`);
    const events = array(page, 'events');
    return events.length ? events[events.length - 1] : null;
  } catch (_) { return null; }
}

function collectTraceEvents(traceId) {
  if (!traceId) return [];
  return Array.from(state.events.values())
    .filter((event) => event.trace_id === traceId)
    .sort((a, b) => Number(a.sequence || 0) - Number(b.sequence || 0));
}

async function loadTrajectory(traceId) {
  if (!traceId) { trajectory.setEvents([]); return; }
  const collected = [];
  let cursor = 0;
  try {
    for (let i = 0; i < 60; i++) {
      const page = await api(`/api/events/page?trace_id=${encodeURIComponent(traceId)}&after=${cursor}&limit=500`);
      const events = array(page, 'events');
      for (const event of events) rememberEvent(event);
      collected.push(...events);
      if (!page.has_more) break;
      cursor = Number(page.next_cursor ?? cursor);
    }
  } catch (error) {
    toast(`轨迹加载失败：${error.message}`, 'error');
  }
  if (traceId === state.traceId) trajectory.setEvents(collected);
}

function rememberEvent(event) {
  if (!event || typeof event !== 'object' || !event.kind) return;
  const identity = `${event.trace_id}/${event.sequence}/${event.span_id}/${event.kind}`;
  state.events.set(identity, event);
  if (state.events.size > 6000) state.events.delete(state.events.keys().next().value);
}

/* ---------- Inspector：对象 / MCP / 技能 ---------- */
function registryCard({ kind, name, description, meta, detail }) {
  return h('details', { class: 'registry-item' },
    h('summary', {}, badge(kind, 'secondary'), h('span', { class: 'mono min-w-0', style: { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' } }, name || '未命名')),
    description ? h('p', { class: 'text-ui-sm text-muted' }, description) : null,
    meta ? h('div', { class: 'text-ui-xs text-subtle mono' }, meta) : null,
    detail ? h('pre', { class: 'registry-detail' }, detail) : null);
}

function renderInspector() {
  const objects = byId('panel-objects');
  objects.replaceChildren(h('div', { class: 'text-ui-xs text-subtle' }, `对象注册表 · ${state.objects.length} 个对象`));
  if (!state.objects.length) objects.append(h('div', { class: 'registry-empty' }, '对象注册表为空。'));
  for (const object of state.objects) {
    const kind = String(object.kind || object.type || 'object');
    objects.append(registryCard({
      kind,
      name: object.id || object.name,
      description: object.description,
      detail: JSON.stringify({ labels: object.labels, dependencies: object.dependencies, capabilities: object.capabilities }, null, 2),
    }));
  }
  const mcp = byId('panel-mcp');
  mcp.replaceChildren(h('div', { class: 'text-ui-xs text-subtle' }, 'MCP 服务与工具'));
  const servers = state.mcp && Array.isArray(state.mcp.servers) ? state.mcp.servers : [];
  if (!servers.length) mcp.append(h('div', { class: 'registry-empty' }, '未发现 MCP 服务。请在运行时配置后刷新。'));
  for (const server of servers) {
    mcp.append(registryCard({
      kind: `MCP · ${server.state || 'unknown'}`,
      name: server.name,
      description: server.error ? String(server.error) : '',
      meta: server.tool_count !== undefined ? `${server.tool_count} 个工具 · ${server.transport || ''}` : '',
      detail: server.server_info ? JSON.stringify(server.server_info, null, 2) : '',
    }));
  }
  const skills = byId('panel-skills');
  skills.replaceChildren(h('div', { class: 'text-ui-xs text-subtle' }, `技能库 · ${state.skills.length} 个技能`));
  if (!state.skills.length) skills.append(h('div', { class: 'registry-empty' }, '运行时会从配置目录加载 SKILL.md。'));
  for (const skill of state.skills) {
    const name = String(skill.name || skill.id || '').replace(/^skill:/, '');
    skills.append(registryCard({
      kind: 'skill',
      name,
      description: skill.description,
      meta: skill.path,
      detail: skill.instructions ? String(skill.instructions).slice(0, 2000) : '',
    }));
  }
}

/* ---------- 任务生命周期 ---------- */
function liveSnapshot(progress) {
  return {
    running: true,
    turn: progress.turn,
    attempt: progress.attempt,
    modelId: progress.model_id,
    reasoning: progress.reasoning || '',
    text: progress.text || '',
    tools: liveTools.tools,
    toolCalls: Array.isArray(progress.tool_calls) ? progress.tool_calls : [],
    startedAt: Number(progress.started_at) || state.task?.startedAt || Date.now(),
  };
}

function applyProgress(progress) {
  const task = state.task;
  if (!task || !progress || String(progress.task_id) !== task.id) return;
  if (task.progress && Number(progress.revision) <= Number(task.progress.revision)) return;
  // 服务端发送累计快照：整体替换，避免重试/重复帧导致文本翻倍。
  task.progress = progress;
  if (progress.session_id && !task.sessionId) {
    task.sessionId = String(progress.session_id);
    state.runningSessions.set(task.sessionId, { taskId: task.id, startedAt: task.startedAt });
    document.dispatchEvent(new CustomEvent('running-changed'));
  }
  if (progress.trace_id && !state.traceId) {
    state.traceId = String(progress.trace_id);
  }
  if (progress.trace_id) loadTrajectory(String(progress.trace_id));
  scheduleProgressRender();
}

function scheduleProgressRender() {
  if (progressTimer) return;
  progressTimer = requestAnimationFrame(() => {
    progressTimer = 0;
    syncTimeline(state.task ? liveSnapshot(state.task.progress || {}) : null);
    updateTaskStateLabel();
  });
}

function updateTaskStateLabel() {
  const label = byId('task-state');
  const halo = byId('task-progress');
  const progress = state.task?.progress;
  if (state.task) {
    const phase = progress?.status === 'streaming' ? '流式输出'
      : progress?.status === 'model_done' ? '处理工具调用' : '执行中';
    label.replaceChildren('● ', phase);
    if (progress?.turn) {
      label.append(' · 第 ', taskTurnRoll, ' 轮');
      rollingNumber(taskTurnRoll, Number(progress.turn));
    }
    label.style.color = 'var(--brand)';
    // HaloProgress：第 N 轮 / max_turns（服务端 limits 未返回时不定态）。
    setHalo(halo, Number(progress?.turn || 0), Number(state.limits?.max_turns) || 0);
    halo.hidden = false;
  } else {
    label.textContent = '就绪';
    label.style.color = '';
    halo.hidden = true;
  }
}

async function refreshSessions() {
  try {
    state.sessions = await api('/api/sessions');
    sidebar.refresh();
    updateHeader();
  } catch (_) { /* 顶栏连接状态会提示 */ }
}

async function finishTask(response, task) {
  if (state.task !== task) return;
  const status = response?.status || 'failed';
  const result = response?.result || null;
  state.task = null;
  clearTimeout(pollTimer);
  pollTimer = 0;
  composer.setRunning(false);
  if (task.sessionId) state.runningSessions.delete(task.sessionId);
  document.dispatchEvent(new CustomEvent('running-changed'));

  const resultSession = result?.session_id || task.sessionId;
  if (resultSession) {
    if (state.currentSessionId !== String(resultSession)) {
      // 草稿视图发出的任务落库成会话后，切换到会话视图（否则最终消息不会出现在时间线）。
      await loadSession(String(resultSession), { force: true });
    } else {
      await reloadSessionMessages(String(resultSession));
    }
  }
  if (status === 'cancelled') toast('任务已取消');
  if (status === 'failed') {
    const reason = response?.error || result?.error || '未知错误';
    // 失败时服务端通常没有 assistant 消息，客户端补一条注记让失败原因留在时间线里。
    messagesOf(task.viewKey).push({ role: 'error', content: `任务失败：${reason}`, timestamp_ms: Date.now() });
    toast(reason, 'error');
  }
  if (result?.trace_id) { state.traceId = String(result.trace_id); loadTrajectory(String(result.trace_id)); }
  syncTimeline();
  updateTaskStateLabel();
  updateHeader();
  await refreshSessions();
}

async function reloadSessionMessages(sessionId) {
  try {
    const detail = await api(`/api/sessions/${encodeURIComponent(sessionId)}`);
    state.transcripts.set(viewKeyOf(sessionId), array(detail?.messages, 'messages'));
  } catch (_) { /* 保留现有缓存 */ }
}

async function pollTask(task) {
  if (state.task !== task || !task.id) return;
  try {
    const response = await api(`/api/tasks/${encodeURIComponent(task.id)}`);
    if (state.task !== task) return;
    if (response.progress) applyProgress(response.progress);
    if (['completed', 'failed', 'cancelled'].includes(response.status)) {
      await finishTask(response, task);
      return;
    }
    pollTimer = setTimeout(() => pollTask(task), 900);
  } catch (error) {
    if (state.task !== task) return;
    if (error.status === 404) {
      await finishTask({ status: 'failed', error: '任务索引已不可用，服务可能已重启。' }, task);
      return;
    }
    pollTimer = setTimeout(() => pollTask(task), 2000);
  }
}

async function submitPrompt(prompt) {
  const draftKeyValue = state.currentSessionId ? viewKeyOf(state.currentSessionId) : draftKey();
  state.draftKeyValue = draftKeyValue;
  messagesOf(draftKeyValue).push({ role: 'user', content: prompt, timestamp_ms: Date.now() });
  syncTimeline();
  const body = {
    session_id: state.currentSessionId,
    prompt,
    profile: {
      automatic: state.request.automatic,
      complexity: state.request.complexity,
      tier: state.request.tier || null,
      model: state.request.model || null,
    },
    skills: Array.from(state.selectedSkills),
  };
  try {
    if (state.currentSessionId) {
      const summary = state.sessions.find((s) => s?.id === state.currentSessionId);
      if (summary?.archived) await apiPatch(`/api/sessions/${encodeURIComponent(state.currentSessionId)}`, { archived: false });
    }
    const submission = await apiPost('/api/run', body);
    if (!submission?.task_id) throw new Error('运行时没有返回 task_id');
    const task = {
      id: String(submission.task_id),
      sessionId: state.currentSessionId || null,
      viewKey: draftKeyValue,
      prompt,
      startedAt: Date.now(),
      progress: null,
    };
    state.task = task;
    state.traceId = '';
    liveTools.tools.clear();
    trajectory.setEvents([]);
    composer.setRunning(true);
    updateTaskStateLabel();
    pollTask(task);
    return null;
  } catch (error) {
    toast(`任务提交失败：${error.message}`, 'error');
    // 回滚乐观追加的用户消息。
    const messages = messagesOf(draftKeyValue);
    if (messages.at(-1)?.role === 'user' && messages.at(-1)?.content === prompt) messages.pop();
    syncTimeline();
    return error;
  }
}

async function controlTask(prompt, kind) {
  const task = state.task;
  if (!task?.id) throw new Error('任务尚未就绪');
  const queued = await apiPost(`/api/tasks/${encodeURIComponent(task.id)}/${kind}`, { prompt });
  messagesOf(task.viewKey).push({ role: 'system', content: `${kind === 'steer' ? '已排队调整方向' : '已排队追加任务'}：${prompt}（队列序号 ${queued.sequence}）` });
  syncTimeline();
  return queued;
}

async function cancelTask() {
  const task = state.task;
  if (!task?.id) return;
  try { await apiPost(`/api/tasks/${encodeURIComponent(task.id)}/cancel`); }
  catch (error) { toast(`取消任务失败：${error.message}`, 'error'); }
}

/* ---------- SSE ---------- */
function attachStreams() {
  for (const source of streamSources) source.close();
  streamSources = [];
  if (!window.EventSource) { dock.statusEl.textContent = '事件流 / 浏览器不支持'; return; }
  const sources = [
    openSse('/api/events/stream', {
      onOpen: () => dock.statusEl.textContent = '事件流 / 已连接',
      onError: () => dock.statusEl.textContent = '事件流 / 重连中',
      onTrace: (event) => {
        rememberEvent(event);
        if (['tool.start', 'tool.result'].includes(event.kind)) {
          liveTools.apply(event);
          scheduleProgressRender();
        }
        if (state.traceId && event.trace_id === state.traceId) {
          trajectory.setEvents(collectTraceEvents(state.traceId));
        }
      },
      onGap: () => { if (state.traceId) loadTrajectory(state.traceId); },
    }),
    openSse('/api/live/stream', {
      onProgress: (progress) => applyProgress(progress),
      onGap: () => { const task = state.task; if (task?.id) pollTask(task); },
      onOpen: () => { const task = state.task; if (task?.id) pollTask(task); },
    }),
  ];
  streamSources = sources;
}

/* ---------- 工作区切换 ---------- */
async function loadWorkspaceData() {
  const names = ['状态', '模型', '会话', '项目', '对象', '技能', '界面'];
  const results = await Promise.allSettled(
    ['/api/state', '/api/models', '/api/sessions', '/api/projects', '/api/objects', '/api/skills', '/api/ui'].map((path) => api(path)));
  const value = (i) => results[i].status === 'fulfilled' ? results[i].value : null;

  const runtime = value(0);
  state.simulation = Boolean(runtime?.simulation);
  state.mcp = runtime?.mcp || null;
  state.limits = runtime?.limits || null;
  state.tiers = array(runtime?.tiers);
  state.workspace = typeof runtime?.workspace === 'string'
    ? runtime.workspace : runtime?.workspace?.path || '本地工作区';
  const workspaceLabel = state.workspace.replace(/^\\\\\?\\/, '').replace(/[\\/]+$/, '').split(/[\\/]/).pop() || state.workspace;
  byId('workspace-name').textContent = state.simulation ? `${workspaceLabel} · 模拟模式` : workspaceLabel;
  byId('workspace-name').title = `${workspaceLabel} · ${state.workspace}`;
  state.models = array(value(1), 'models').filter((m) => m && m.id);
  state.sessions = array(value(2), 'sessions');
  state.projects = array(value(3)).filter((p) => p && p.id);
  state.objects = array(value(4), 'objects');
  state.skills = array(value(5), 'skills');
  if (value(6) && typeof value(6) === 'object') state.ui = { ...state.ui, ...value(6) };
  applyTheme(state.ui.theme);

  const failed = results.map((r, i) => r.status === 'rejected' ? names[i] : null).filter(Boolean);
  if (failed.length) toast(`无法读取：${failed.join('、')}。请确认本地服务已启动。`, 'error');
  setConnection(results.some((r) => r.status === 'fulfilled'));

  sidebar.refresh();
  updateHeader();
  syncTimeline();
  renderInspector();
}

async function switchWorkspace(id, { push = true } = {}) {
  const target = id || 'default';
  if (target === currentWorkspaceId) { sidebar.reloadWorkspaces(target); return; }
  if (state.task) {
    toast('当前工作区有任务运行中，等它结束或取消后再切换', 'error');
    sidebar.reloadWorkspaces(currentWorkspaceId);
    return;
  }
  const prefix = target === 'default' ? '' : `/w/${encodeURIComponent(target)}`;
  setWorkspacePrefix(prefix);
  if (push) history.pushState(null, '', prefix ? `${prefix}/` : '/');
  wsSessionMemory.set(currentWorkspaceId, state.currentSessionId);
  currentWorkspaceId = target;
  state.currentSessionId = wsSessionMemory.get(target) ?? null;
  state.draftKeyValue = draftKey();
  state.traceId = '';
  state.events.clear();
  state.transcripts.clear();
  state.runningSessions.clear();
  liveTools.tools.clear();
  trajectory.setEvents([]);
  timeline.reset();
  updateTaskStateLabel();
  await loadWorkspaceData();
  sidebar.setCurrent(state.currentSessionId);
  sidebar.reloadWorkspaces(target);
  attachStreams();
  // 设备面板随前缀换数据：激活中的面板立即重列，终端切到该工作区的会话组。
  await dock.refreshActive();
  updateHeader();
}

/* ---------- 设置 ---------- */
function settingsDialog() {
  const theme = h('select', { 'data-component': 'input', 'aria-label': '主题' },
    h('option', { value: 'dark' }, '深色'),
    h('option', { value: 'light' }, '浅色'),
    h('option', { value: 'system' }, '跟随系统'));
  theme.value = state.ui.theme;
  const submitKey = h('input', { 'data-component': 'input', value: state.ui.submit_key, maxlength: '40', 'aria-label': '发送快捷键' });
  const newKey = h('input', { 'data-component': 'input', value: state.ui.new_session_key, maxlength: '40', 'aria-label': '新建会话快捷键' });
  const cancelKey = h('input', { 'data-component': 'input', value: state.ui.cancel_key, maxlength: '40', 'aria-label': '取消任务快捷键' });
  const row = (label, el) => h('label', { class: 'flex flex-col gap-1' }, h('span', { 'data-component': 'label' }, label), el);
  const dialog = openDialog({
    title: '界面与快捷键',
    body: h('div', { class: 'flex flex-col gap-3' },
      row('主题', theme),
      row('发送任务', submitKey),
      row('新建会话', newKey),
      row('取消任务', cancelKey)),
    footer: [
      button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
      button('保存', {
        'data-variant': 'default',
        onclick: async () => {
          const value = {
            theme: theme.value,
            submit_key: submitKey.value.trim(),
            new_session_key: newKey.value.trim(),
            cancel_key: cancelKey.value.trim(),
          };
          if (!value.submit_key || !value.new_session_key || !value.cancel_key) {
            toast('快捷键不能为空', 'error');
            return;
          }
          try {
            const saved = await api('/api/ui', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value) });
            state.ui = { ...state.ui, ...saved };
            applyTheme(state.ui.theme);
            dialog.close();
            toast('设置已保存到当前工作区');
          } catch (error) { toast(`保存失败：${error.message}`, 'error'); }
        },
      }),
    ],
  });
}

/* ---------- 快捷键 ---------- */
function matchesShortcut(event, shortcut) {
  if (event.isComposing || event.repeat) return false;
  const parts = String(shortcut || '').toLowerCase().split('+').map((p) => p.trim());
  const key = parts.pop();
  const ctrl = parts.includes('ctrl') || parts.includes('control');
  const meta = parts.includes('meta') || parts.includes('cmd') || parts.includes('command');
  const shift = parts.includes('shift');
  const alt = parts.includes('alt');
  const command = ctrl && !meta ? event.ctrlKey || event.metaKey : event.ctrlKey === ctrl && event.metaKey === meta;
  return command && event.altKey === alt && event.shiftKey === shift && event.key.toLowerCase() === key;
}

function openModelQuickSelect() {
  const anchor = document.querySelector('.model-chip') || document.querySelector('.composer-bar');
  if (!anchor) return;
  import('./model-select.js').then(({ openModelSelect }) => {
    openModelSelect(anchor, state, {
      onPick: () => syncTimeline(),
      restoreFocus: () => composer?.focus(),
    });
  });
}

/* ---------- 装配 ---------- */
function newSession() {
  state.draftKeyValue = draftKey();
  state.currentSessionId = null;
  wsSessionMemory.set(currentWorkspaceId, null);
  state.traceId = '';
  state.requestCounter++;
  sidebar.setCurrent(null);
  timeline.reset();
  syncTimeline();
  updateHeader();
  trajectory.setEvents([]);
  composer?.focus();
}

async function bootstrap() {
  const devicePanels = {
    files: files = createFilesPanel({ container: byId('panel-files') }),
    git: git = createGitPanel({ container: byId('panel-git') }),
    term: terminal = createTerminalPane({ container: byId('panel-term') }),
    browser: browser = createBrowserPane({ container: byId('panel-browser') }),
  };
  dock = createDock({
    devicePanels,
    // 检查组的 DOM 可见性由 dock 统一切换（#panel-* 的 hidden）。
    inspectPanels: {
      trajectory: { show() {}, hide() {} },
      agents: agentsPanel = createAgentsPanel({ container: byId('panel-agents') }),
      tasks: createTasksPanel({ container: byId('panel-tasks') }),
      objects: { show() {}, hide() {} },
      mcp: { show() {}, hide() {} },
      skills: { show() {}, hide() {} },
      models: modelsPanel = createModelsPanel({ container: byId('panel-models') }),
    },
    defaultDevice: 'files',
  });
  dock.statusEl.id = 'stream-status';
  byId('dock-head').replaceChildren(dock.el);

  sidebar = createWorkbenchSidebar({
    container: byId('sidebar'),
    onSelect: (id, { forceNew } = {}) => (id ? loadSession(id) : newSession()),
    onNewSession: newSession,
    onSwitchWorkspace: (id) => switchWorkspace(id),
  });
  composer = createComposer({
    container: byId('composer-host'),
    onSubmit: submitPrompt,
    onControl: controlTask,
    onCancel: cancelTask,
  });

  window.addEventListener('popstate', () => {
    const id = workspaceIdFromLocation();
    if (id !== currentWorkspaceId) switchWorkspace(id, { push: false });
  });

  await loadWorkspaceData();
  sidebar.setCurrent(state.currentSessionId);
  dock.activate('trajectory');
  attachStreams();
  composer.focus();

  byId('fork-session').addEventListener('click', async () => {
    if (!state.currentSessionId) return;
    try {
      const fork = await apiPost(`/api/sessions/${encodeURIComponent(state.currentSessionId)}/fork`, {});
      await refreshSessions();
      await loadSession(fork.id);
      toast(`已创建分支：${fork.title || fork.id}`);
    } catch (error) { toast(`分支失败：${error.message}`, 'error'); }
  });
  byId('refresh-sessions').addEventListener('click', () => refreshSessions());
  byId('toggle-dock').addEventListener('click', () => {
    const open = document.body.classList.toggle('dock-open');
    byId('toggle-dock').setAttribute('aria-pressed', String(open));
  });
  byId('theme-toggle').addEventListener('click', () => {
    const order = ['dark', 'light', 'system'];
    const next = order[(order.indexOf(state.ui.theme) + 1) % order.length];
    applyTheme(next);
    api('/api/ui', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...state.ui, theme: next }) })
      .catch(() => { /* 主题切换本地即时生效，持久化失败不阻断 */ });
  });
  byId('open-settings').addEventListener('click', settingsDialog);

  document.addEventListener('keydown', (event) => {
    if (matchesShortcut(event, state.ui.new_session_key)) { event.preventDefault(); newSession(); }
    else if (matchesShortcut(event, state.ui.cancel_key)) { event.preventDefault(); cancelTask(); }
    else if (matchesShortcut(event, 'Ctrl+M')) { event.preventDefault(); openModelQuickSelect(); }
  });
  document.addEventListener('running-changed', () => { composer.update(); });
  // 子代理面板「打开会话」：切到对应会话（任务运行中也能跟踪）。
  document.addEventListener('agents-open-session', (event) => {
    const sessionId = event.detail?.sessionId;
    if (sessionId) loadSession(sessionId);
  });
  // Enter 直发在 composer 内部处理；这里补 Ctrl/自定义发送键。
  document.addEventListener('keydown', (event) => {
    if (event.target !== document.querySelector('.composer-input')) return;
    if (matchesShortcut(event, state.ui.submit_key)) {
      event.preventDefault();
      document.querySelector('.composer form')?.requestSubmit();
    }
  });
}

bootstrap().catch((error) => {
  setConnection(false);
  toast(`运行时初始化失败：${error.message}`, 'error');
});
