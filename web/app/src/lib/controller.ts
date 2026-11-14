// 工作台控制器：任务生命周期、SSE 双流、工作区切换、会话加载与轨迹分页。
// 与 React 组件解耦：组件只读 store，并调用这里导出的动作。

import {
  api, apiPatch, apiPost, apiPut, apiRoot, array, openSse, setWorkspacePrefix, workspaceIdFromLocation, ApiError,
} from './api';
import {
  getState, setState, nextDraftKey, viewKeyOf, currentViewKey, requestSummary,
  type PanelName, type Session, type Task, type UiSettings, DEVICE_TABS,
} from './store';
import { applyLiveToolEvent, type LiveTool, type Message, type TraceEvent } from './projection';
import { bridge } from './bridge';
import { toast } from '@/components/ui/toaster';

/* eslint-disable @typescript-eslint/no-explicit-any */

let pollTimer = 0;
let progressFrame = 0;
let pendingProgress: any = null;
let streamSources: EventSource[] = [];
// 每个工作区记住上次打开的会话，切回时恢复。
const wsSessionMemory = new Map<string, string | null>([['default', null]]);
// 轨迹分页加载去重：traceId -> { cursor, done, promise }。同一 trace 的进行中加载
// 复用同一 Promise，已完成且非补洞时不再重复分页。
const trajectoryLoads = new Map<string, { cursor: number; done: boolean; promise: Promise<void> | null }>();
// 工作区加载代际：响应返回时代际不匹配即整组丢弃，防止快速切换时旧数据覆盖当前视图。
let workspaceGen = 0;
// 会话加载竞态守卫。
let requestCounter = 0;
// 任务提交锁：/api/run 返回前不允许第二次提交（避免启动多个未追踪任务）。
let submitting = false;
// 新草稿期望归属的项目（从项目行发起时设置），落库后自动写 project_id。
let pendingProjectId: string | null = null;
let lastAnnouncedPhase = '';
// SSE trace 事件缓存（identity -> event），切换会话时用于重建当前轨迹。
const events = new Map<string, TraceEvent>();
// 轨迹去重：当前轨迹面板已收录的事件身份。
const seenEvents = new Set<string>();

const eventKey = (event: TraceEvent) => `${event?.trace_id}/${event?.sequence}/${event?.span_id}/${event?.kind}`;

/* ---------- 主题 ---------- */
function resolveTheme(theme: string) {
  if (theme !== 'system') return theme;
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}
export function applyTheme(theme: UiSettings['theme']) {
  setState((s) => ({ ui: { ...s.ui, theme } }));
  document.documentElement.classList.toggle('dark', resolveTheme(theme) === 'dark');
  document.documentElement.style.colorScheme = resolveTheme(theme);
}
window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
  if (getState().ui.theme === 'system') applyTheme('system');
});

export async function cycleTheme() {
  const order: UiSettings['theme'][] = ['dark', 'light', 'system'];
  const ui = getState().ui;
  const next = order[(order.indexOf(ui.theme) + 1) % order.length];
  applyTheme(next);
  // 主题切换本地即时生效，持久化失败不阻断。
  apiPut('/api/ui', { ...ui, theme: next }).catch(() => {});
}

export async function saveUiSettings(value: UiSettings) {
  const saved = await apiPut<UiSettings>('/api/ui', value);
  setState((s) => ({ ui: { ...s.ui, ...saved } }));
  applyTheme((saved?.theme || value.theme) as UiSettings['theme']);
}

export function announce(text: string) {
  setState({ announcement: text });
}

/* ---------- transcripts ---------- */
const messagesOf = (key: string) => getState().transcripts[key] || [];
function setMessages(key: string, messages: Message[]) {
  setState((s) => ({ transcripts: { ...s.transcripts, [key]: messages } }));
}
function pushMessage(key: string, message: Message) {
  setMessages(key, [...messagesOf(key), message]);
}

/* ---------- 轨迹 ---------- */
function setTrace(next: TraceEvent[]) {
  seenEvents.clear();
  for (const event of next) seenEvents.add(eventKey(event));
  setState({ trace: next });
}

// 增量追加（SSE / 分页补齐）：身份去重；序号回退（补历史页）时整体重排。
export function appendEvents(incoming: TraceEvent[]) {
  const fresh: TraceEvent[] = [];
  for (const event of incoming || []) {
    if (!event || typeof event !== 'object' || !event.kind) continue;
    const key = eventKey(event);
    if (seenEvents.has(key)) continue;
    seenEvents.add(key);
    fresh.push(event);
  }
  if (!fresh.length) return;
  const current = getState().trace;
  const prevLast = current.at(-1);
  const merged = [...current, ...fresh];
  if (prevLast && Number(fresh[0].sequence || 0) < Number(prevLast.sequence || 0)) {
    merged.sort((a, b) => Number(a.sequence || 0) - Number(b.sequence || 0));
  }
  setState({ trace: merged });
}

function collectTraceEvents(traceId: string) {
  if (!traceId) return [];
  return Array.from(events.values())
    .filter((event) => event.trace_id === traceId)
    .sort((a, b) => Number(a.sequence || 0) - Number(b.sequence || 0));
}

function rememberEvent(event: TraceEvent) {
  if (!event || typeof event !== 'object' || !event.kind) return;
  events.set(eventKey(event), event);
  if (events.size > 6000) events.delete(events.keys().next().value as string);
}

async function loadTrajectory(traceId: string, { resume = false } = {}) {
  if (!traceId) { setTrace([]); return; }
  let entry = trajectoryLoads.get(traceId);
  if (!entry) {
    entry = { cursor: 0, done: false, promise: null };
    trajectoryLoads.set(traceId, entry);
  }
  if (entry.promise) return entry.promise;
  if (entry.done && !resume) return;
  const target = entry;
  const run = (async () => {
    let cursor = resume ? target.cursor : 0;
    try {
      for (let i = 0; i < 60; i++) {
        const page = await api(`/api/events/page?trace_id=${encodeURIComponent(traceId)}&after=${cursor}&limit=500`);
        const pageEvents = array<TraceEvent>(page, 'events');
        if (traceId === getState().traceId && pageEvents.length) appendEvents(pageEvents);
        if (!page.has_more) { target.cursor = cursor; target.done = true; break; }
        cursor = Number(page.next_cursor ?? cursor);
        target.cursor = cursor;
      }
    } catch (error) {
      toast(`轨迹加载失败：${(error as Error).message}`, 'error');
    }
  })();
  target.promise = run;
  try { await run; } finally {
    if (trajectoryLoads.get(traceId)?.promise === run) target.promise = null;
  }
}

async function latestSessionTrace(sessionId: string) {
  try {
    const page = await api(`/api/events/page?session_id=${encodeURIComponent(sessionId)}&limit=500`);
    const list = array<TraceEvent>(page, 'events');
    return list.length ? list[list.length - 1] : null;
  } catch { return null; }
}

/* ---------- 会话 ---------- */
export async function refreshSessions() {
  try {
    const sessions = await api<Session[]>('/api/sessions');
    setState({ sessions: array(sessions, 'sessions') });
  } catch { /* 顶栏连接状态会提示 */ }
}

export async function refreshProjects() {
  try {
    const projects = await api('/api/projects');
    setState({ projects: array(projects).filter((p: any) => p && p.id) });
  } catch { /* 连接错误由顶栏状态指示 */ }
}

export async function refreshSidebar() {
  await Promise.all([refreshProjects(), refreshSessions()]);
}

export async function loadSession(id: string) {
  const request = ++requestCounter;
  wsSessionMemory.set(getState().currentWorkspaceId, id);
  const key = viewKeyOf(id);
  setState((s) => ({
    currentSessionId: id,
    sidebarOpen: false,
    transcripts: s.transcripts[key] ? s.transcripts : { ...s.transcripts, [key]: [] },
  }));
  // 活动设备面板是终端/浏览器时，让附属绑定跟随当前会话（重连/恢复）。
  const panel = getState().activePanel;
  if (panel === 'term') bridge.terminal?.show({ sessionId: id }).catch(() => {});
  if (panel === 'browser') bridge.browser?.show({ sessionId: id }).catch(() => {});
  try {
    const detail = await api(`/api/sessions/${encodeURIComponent(id)}`);
    if (request !== requestCounter) return;
    setMessages(key, array(detail?.messages, 'messages'));
    const latest = await latestSessionTrace(id);
    if (request !== requestCounter) return;
    const traceId = latest?.trace_id || '';
    setState({ traceId });
    // 会话视图整体替换轨迹面板，历史加载器随之失效。
    trajectoryLoads.clear();
    setTrace(collectTraceEvents(traceId));
    if (traceId) loadTrajectory(traceId);
  } catch (error) {
    if (request === requestCounter) toast(`无法读取会话：${(error as Error).message}`, 'error');
  }
}

export function newSession({ projectId }: { projectId?: string | null } = {}) {
  pendingProjectId = projectId || null;
  requestCounter++;
  wsSessionMemory.set(getState().currentWorkspaceId, null);
  trajectoryLoads.clear();
  setState({ currentSessionId: null, draftKey: nextDraftKey(), traceId: '', sidebarOpen: false });
  setTrace([]);
  document.dispatchEvent(new Event('composer-focus'));
}

export async function forkSession(sessionId: string) {
  try {
    const fork = await apiPost(`/api/sessions/${encodeURIComponent(sessionId)}/fork`, {});
    await refreshSessions();
    await loadSession(fork.id);
    toast(`已创建分支：${fork.title || fork.id}`);
  } catch (error) { toast(`分支失败：${(error as Error).message}`, 'error'); }
}

async function reloadSessionMessages(sessionId: string) {
  try {
    const detail = await api(`/api/sessions/${encodeURIComponent(sessionId)}`);
    setMessages(viewKeyOf(sessionId), array(detail?.messages, 'messages'));
  } catch { /* 保留现有缓存 */ }
}

/* ---------- 任务生命周期 ---------- */
function applyProgress(progress: any) {
  const task = getState().task;
  if (!task || !progress || String(progress.task_id) !== task.id) return;
  const latest = pendingProgress && String(pendingProgress.task_id) === task.id ? pendingProgress : task.progress;
  if (latest && Number(progress.revision) <= Number(latest.revision)) return;
  // 服务端发送累计快照：整体替换，避免重试/重复帧导致文本翻倍。
  pendingProgress = progress;
  if (progress.session_id && !task.sessionId) {
    const sessionId = String(progress.session_id);
    setState((s) => ({
      task: s.task && { ...s.task, sessionId },
      runningSessions: { ...s.runningSessions, [sessionId]: { taskId: task.id, startedAt: task.startedAt } },
    }));
  }
  if (progress.trace_id && !getState().traceId) setState({ traceId: String(progress.trace_id) });
  if (progress.trace_id) {
    const traceKey = String(progress.trace_id);
    // 首次发现该 trace 才启动加载；后续帧交给 SSE 增量与完成态补洞。
    if (!trajectoryLoads.has(traceKey)) loadTrajectory(traceKey);
  }
  scheduleProgressRender();
}

function scheduleProgressRender() {
  if (progressFrame) return;
  progressFrame = requestAnimationFrame(() => {
    progressFrame = 0;
    const progress = pendingProgress;
    pendingProgress = null;
    if (progress) setState((s) => (s.task && String(progress.task_id) === s.task.id ? { task: { ...s.task, progress } } : {}));
    const phase = taskPhase(getState().task);
    if (phase && phase !== lastAnnouncedPhase) {
      lastAnnouncedPhase = phase;
      announce(`任务状态：${phase}`);
    }
  });
}

export function taskPhase(task: Task | null) {
  if (!task) return '';
  const status = task.progress?.status;
  return status === 'streaming' ? '流式输出' : status === 'model_done' ? '处理工具调用' : '执行中';
}

async function finishTask(response: any, task: Task) {
  if (getState().task?.id !== task.id) return;
  const status = response?.status || 'failed';
  const result = response?.result || null;
  const sessionId = getState().task?.sessionId || task.sessionId;
  clearTimeout(pollTimer);
  pollTimer = 0;
  lastAnnouncedPhase = '';
  setState((s) => {
    const runningSessions = { ...s.runningSessions };
    if (sessionId) delete runningSessions[sessionId];
    return { task: null, runningSessions };
  });

  const resultSession = result?.session_id || sessionId;
  if (resultSession) {
    if (getState().currentSessionId !== String(resultSession)) {
      // 草稿视图发出的任务落库成会话后，切换到会话视图（否则最终消息不会出现在时间线）。
      await loadSession(String(resultSession));
    } else {
      await reloadSessionMessages(String(resultSession));
    }
    if (pendingProjectId) {
      try {
        await apiPatch(`/api/sessions/${encodeURIComponent(String(resultSession))}`, { project_id: pendingProjectId });
        pendingProjectId = null;
      } catch (error) { toast(`归属项目失败：${(error as Error).message}`, 'error'); }
    }
  }
  const reason = response?.error || result?.error || '未知错误';
  if (status === 'cancelled') toast('任务已取消');
  if (status === 'failed') {
    // 失败时服务端通常没有 assistant 消息，补一条注记让失败原因留在时间线里。
    const key = resultSession ? viewKeyOf(String(resultSession)) : task.viewKey;
    pushMessage(key, { role: 'error', content: `任务失败：${reason}`, timestamp_ms: Date.now() });
    toast(reason, 'error');
  }
  announce(status === 'cancelled' ? '任务已取消' : status === 'failed' ? `任务失败：${reason}` : '任务已完成');
  if (result?.trace_id) {
    setState({ traceId: String(result.trace_id) });
    // 完成态补洞：从已取游标之后拉剩余事件（含 SSE 可能缺失的部分）。
    loadTrajectory(String(result.trace_id), { resume: true });
  }
  await refreshSidebar();
}

async function pollTask(task: Task) {
  if (getState().task?.id !== task.id) return;
  try {
    const response = await api(`/api/tasks/${encodeURIComponent(task.id)}`);
    if (getState().task?.id !== task.id) return;
    if (response.progress) applyProgress(response.progress);
    if (['completed', 'failed', 'cancelled'].includes(response.status)) {
      await finishTask(response, task);
      return;
    }
    clearTimeout(pollTimer);
    pollTimer = window.setTimeout(() => pollTask(task), 900);
  } catch (error) {
    if (getState().task?.id !== task.id) return;
    if (error instanceof ApiError && error.status === 404) {
      await finishTask({ status: 'failed', error: '任务索引已不可用，服务可能已重启。' }, task);
      return;
    }
    clearTimeout(pollTimer);
    pollTimer = window.setTimeout(() => pollTask(task), 2000);
  }
}

/** 返回 null 表示已提交；返回 Error 表示失败（composer 据此回填输入）。 */
export async function submitPrompt(prompt: string): Promise<Error | null> {
  if (submitting) {
    toast('上一个任务仍在提交，请稍候', 'error');
    return new Error('busy');
  }
  submitting = true;
  try {
    return await submitPromptInner(prompt);
  } finally {
    submitting = false;
  }
}

async function submitPromptInner(prompt: string): Promise<Error | null> {
  const s = getState();
  const viewKey = currentViewKey(s);
  pushMessage(viewKey, { role: 'user', content: prompt, timestamp_ms: Date.now() });
  const body = {
    session_id: s.currentSessionId,
    prompt,
    profile: {
      automatic: s.request.automatic,
      complexity: s.request.complexity,
      tier: s.request.tier || null,
      model: s.request.model || null,
    },
    skills: s.selectedSkills,
  };
  try {
    if (s.currentSessionId) {
      const summary = s.sessions.find((x) => x?.id === s.currentSessionId);
      if (summary?.archived) await apiPatch(`/api/sessions/${encodeURIComponent(s.currentSessionId)}`, { archived: false });
    }
    const submission = await apiPost('/api/run', body);
    if (!submission?.task_id) throw new Error('运行时没有返回 task_id');
    const task: Task = {
      id: String(submission.task_id),
      sessionId: s.currentSessionId || null,
      viewKey,
      prompt,
      startedAt: Date.now(),
      progress: null,
      cancelRequested: false,
    };
    trajectoryLoads.clear();
    pendingProgress = null;
    setState((prev) => ({
      task,
      traceId: '',
      liveTools: {},
      runningSessions: task.sessionId
        ? { ...prev.runningSessions, [task.sessionId]: { taskId: task.id, startedAt: task.startedAt } }
        : prev.runningSessions,
    }));
    setTrace([]);
    pollTask(task);
    return null;
  } catch (error) {
    toast(`任务提交失败：${(error as Error).message}`, 'error');
    // 回滚乐观追加的用户消息。
    const messages = messagesOf(viewKey);
    const last = messages.at(-1);
    if (last?.role === 'user' && last?.content === prompt) setMessages(viewKey, messages.slice(0, -1));
    return error as Error;
  }
}

export async function controlTask(prompt: string, kind: 'steer' | 'follow-up') {
  const task = getState().task;
  if (!task?.id) throw new Error('任务尚未就绪');
  const queued = await apiPost(`/api/tasks/${encodeURIComponent(task.id)}/${kind}`, { prompt });
  pushMessage(task.viewKey, {
    role: 'system-note',
    content: `${kind === 'steer' ? '已排队调整方向' : '已排队追加任务'}：${prompt}（队列序号 ${queued.sequence}）`,
  });
  return queued;
}

export async function cancelTask() {
  const task = getState().task;
  if (!task?.id) return;
  setState((s) => ({ task: s.task && { ...s.task, cancelRequested: true } }));
  try { await apiPost(`/api/tasks/${encodeURIComponent(task.id)}/cancel`); }
  catch (error) { toast(`取消任务失败：${(error as Error).message}`, 'error'); }
}

export async function cancelTaskById(taskId: string) {
  await apiPost(`/api/tasks/${encodeURIComponent(taskId)}/cancel`, {});
}

/* ---------- SSE ---------- */
function attachStreams() {
  for (const source of streamSources) source.close();
  streamSources = [];
  if (!window.EventSource) { setState({ streamStatus: '事件流 / 浏览器不支持' }); return; }
  streamSources = [
    openSse('/api/events/stream', {
      onOpen: () => setState({ streamStatus: '事件流 / 已连接' }),
      onError: () => setState({ streamStatus: '事件流 / 重连中' }),
      onTrace: (event: TraceEvent) => {
        rememberEvent(event);
        if (event.kind === 'tool.start' || event.kind === 'tool.result') {
          const tools = new Map<string, LiveTool>(Object.entries(getState().liveTools));
          applyLiveToolEvent(tools, event);
          setState({ liveTools: Object.fromEntries(tools) });
        }
        // 只把影响投影的模型事件增量送入轨迹面板（工具状态由 live 工具区呈现）。
        const traceId = getState().traceId;
        if (traceId && event.trace_id === traceId
          && ['model.start', 'model.response', 'model.error'].includes(event.kind)) {
          appendEvents([event]);
        }
      },
      onGap: () => { const traceId = getState().traceId; if (traceId) loadTrajectory(traceId); },
    }),
    openSse('/api/live/stream', {
      onProgress: (progress) => applyProgress(progress),
      onGap: () => { const task = getState().task; if (task?.id) pollTask(task); },
      onOpen: () => { const task = getState().task; if (task?.id) pollTask(task); },
    }),
  ];
}

/* ---------- 工作区 ---------- */
export async function reloadWorkspaces() {
  try {
    const list = await apiRoot('/api/workspaces');
    const records = array(list, 'workspaces');
    setState((s) => {
      const exists = records.some((r: any) => r.id === s.currentWorkspaceId);
      return { workspaces: records, currentWorkspaceId: exists ? s.currentWorkspaceId : records[0]?.id || 'default' };
    });
  } catch {
    toast('无法读取工作区列表', 'error');
  }
}

async function loadWorkspaceData() {
  const gen = ++workspaceGen;
  const names = ['状态', '模型', '会话', '项目', '对象', '技能', '界面'];
  const results = await Promise.allSettled(
    ['/api/state', '/api/models', '/api/sessions', '/api/projects', '/api/objects', '/api/skills', '/api/ui'].map((path) => api(path)));
  // 响应返回时代际已推进：这是被更新的切换取代的旧工作区数据，整组丢弃。
  if (gen !== workspaceGen) return;
  const value = (i: number) => (results[i].status === 'fulfilled' ? (results[i] as PromiseFulfilledResult<any>).value : null);

  const runtime = value(0);
  const workspacePath = typeof runtime?.workspace === 'string'
    ? runtime.workspace : runtime?.workspace?.path || '本地工作区';
  const configuredName = typeof runtime?.display_name === 'string' ? runtime.display_name.trim() : '';
  const workspaceLabel = configuredName
    || workspacePath.replace(/^\\\\\?\\/, '').replace(/[\\/]+$/, '').split(/[\\/]/).pop() || workspacePath;
  const ui = value(6) && typeof value(6) === 'object' ? { ...getState().ui, ...value(6) } : getState().ui;
  setState({
    simulation: Boolean(runtime?.simulation),
    mcp: runtime?.mcp || null,
    limits: runtime?.limits || null,
    tiers: array(runtime?.tiers),
    workspacePath,
    workspaceLabel,
    models: array(value(1), 'models').filter((m: any) => m && m.id),
    sessions: array(value(2), 'sessions'),
    projects: array(value(3)).filter((p: any) => p && p.id),
    objects: array(value(4), 'objects'),
    skills: array(value(5), 'skills'),
    ui,
    apiOnline: results.some((r) => r.status === 'fulfilled'),
  });
  applyTheme(ui.theme);
  const failed = results.map((r, i) => (r.status === 'rejected' ? names[i] : null)).filter(Boolean);
  if (failed.length) toast(`无法读取：${failed.join('、')}。请确认本地服务已启动。`, 'error');
}

export async function refreshSkills() {
  try {
    await apiPost('/api/skills/refresh', {});
  } catch { /* 刷新失败时仍尝试读取现有列表 */ }
  try {
    setState({ skills: array(await api('/api/skills'), 'skills') });
    toast('技能列表已刷新');
  } catch (error) { toast(`读取技能失败：${(error as Error).message}`, 'error'); }
}

export async function switchWorkspace(id: string, { push = true } = {}) {
  const target = id || 'default';
  const s = getState();
  setState({ sidebarOpen: false });
  if (target === s.currentWorkspaceId) { reloadWorkspaces(); return; }
  if (s.task) {
    toast('当前工作区有任务运行中，等它结束或取消后再切换', 'error');
    return;
  }
  const prefix = target === 'default' ? '' : `/w/${encodeURIComponent(target)}`;
  setWorkspacePrefix(prefix);
  if (push) history.pushState(null, '', prefix ? `${prefix}/` : '/');
  wsSessionMemory.set(s.currentWorkspaceId, s.currentSessionId);
  trajectoryLoads.clear();
  events.clear();
  requestCounter++;
  setState({
    currentWorkspaceId: target,
    currentSessionId: wsSessionMemory.get(target) ?? null,
    draftKey: nextDraftKey(),
    traceId: '',
    transcripts: {},
    runningSessions: {},
    liveTools: {},
  });
  setTrace([]);
  await loadWorkspaceData();
  // 本次切换已被更新的切换取代：跳过收尾，避免旧工作区的会话指针与 SSE 覆盖新视图。
  if (getState().currentWorkspaceId !== target) return;
  reloadWorkspaces();
  attachStreams();
  const sessionId = getState().currentSessionId;
  if (sessionId) loadSession(sessionId);
  // 设备面板随前缀换数据：激活中的面板立即重列。
  document.dispatchEvent(new Event('workspace-changed'));
}

/* ---------- 停靠面板 ---------- */
export function activatePanel(name: PanelName) {
  const group = (DEVICE_TABS as string[]).includes(name) ? 'device' : 'inspect';
  setState((s) => ({ activePanel: name, lastByGroup: { ...s.lastByGroup, [group]: name } }));
}

export async function openAttachedTerminal(sessionId: string) {
  activatePanel('term');
  setState({ inspectorOpen: true });
  await bridge.terminal?.open(sessionId);
}

export async function openAttachedBrowser(sessionId: string) {
  activatePanel('browser');
  setState({ inspectorOpen: true });
  await bridge.browser?.open(sessionId);
}

/* ---------- 模型 ---------- */
export function pickModel(value: string) {
  setState((s) => {
    const request = { ...s.request };
    if (value === '__auto__') { request.tier = ''; request.model = ''; }
    else if (value.startsWith('model:')) { request.model = value.slice(6); request.tier = ''; }
    else if (value.startsWith('tier:')) { request.tier = value.slice(5); request.model = ''; }
    return { request };
  });
  toast(`模型选择：${requestSummary(getState().request)}`);
}

/* ---------- 启动 ---------- */
async function loadLanEndpoint() {
  const info = await api('/api/network').catch(() => null);
  if (!info || !Array.isArray(info.addresses) || !info.addresses.length) return;
  setState({ lanUrls: info.addresses.map((ip: string) => `http://${ip}:${info.port}`) });
}

let booted = false;
export async function bootstrap() {
  if (booted) return;
  booted = true;
  setState({ currentWorkspaceId: workspaceIdFromLocation() });
  window.addEventListener('popstate', () => {
    const id = workspaceIdFromLocation();
    if (id !== getState().currentWorkspaceId) switchWorkspace(id, { push: false });
  });
  try {
    await loadWorkspaceData();
    reloadWorkspaces();
    attachStreams();
    loadLanEndpoint();
  } catch (error) {
    setState({ apiOnline: false });
    toast(`运行时初始化失败：${(error as Error).message}`, 'error');
  }
}
