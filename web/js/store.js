// 全局状态 + 最小事件总线。模块之间通过 bus 通知刷新，避免互相 import 渲染函数。

import { array } from './api.js';

export const tierNames = {
  nano: '轻量', fast: '快速', balanced: '均衡',
  reasoning: '推理', frontier: '前沿', specialist: '专家',
};

export const state = {
  apiOnline: false,
  simulation: false,
  models: [],
  tiers: [],
  sessions: [],
  projects: [],
  objects: [],
  skills: [],
  mcp: null,
  workspace: '',
  limits: null,                   // { max_turns, max_concurrent_tasks }（HaloProgress 确定态）
  ui: { theme: 'dark', submit_key: 'Ctrl+Enter', new_session_key: 'Alt+N', cancel_key: 'Ctrl+K' },

  currentSessionId: null,          // null = 草稿
  draftCounter: 0,
  transcripts: new Map(),          // viewKey('session:<id>'|'draft:<n>') -> messages[]
  requestCounter: 0,               // 会话加载竞态守卫

  task: null,                      // { id, viewKey, prompt, cancelRequested, sendingControl, failures }
  runningSessions: new Map(),      // sessionId -> { taskId, startedAt, prompt }

  selectedSkills: new Set(),
  request: { tier: '', model: '', automatic: true, complexity: 4 },

  traceId: '',
  events: new Map(),               // identity -> trace event
  tracePages: new Map(),           // traceId -> { cursor, hasMore }
  pageLoading: false,
};

export const viewKeyOf = (sessionId) => (sessionId ? `session:${sessionId}` : null);
export const draftKey = () => `draft:${++state.draftCounter}`;
export const currentKey = () => state.currentSessionId ? `session:${state.currentSessionId}` : state.draftKey;

const bus = new EventTarget();
export const on = (name, handler) => { bus.addEventListener(name, handler); };
export const emit = (name, detail) => bus.dispatchEvent(new CustomEvent(name, { detail }));

export const emitChanged = (name) => emit(name);

export function isRunning(sessionId) {
  return sessionId && state.runningSessions.has(sessionId);
}

export function messagesOf(key) {
  if (!state.transcripts.has(key)) state.transcripts.set(key, []);
  return state.transcripts.get(key);
}

export function sessionById(id) {
  return state.sessions.find((s) => s && (s.id === id || typeof s === 'string' && s === id));
}

export function modelById(id) {
  return state.models.find((m) => m && m.id === id) || null;
}

export function applyBootstrap({ runtime, models, sessions, objects, skills, ui }) {
  if (runtime) {
    state.simulation = Boolean(runtime.simulation);
    state.mcp = runtime.mcp || null;
    state.tiers = array(runtime.tiers);
    state.workspace = typeof runtime.workspace === 'string' ? runtime.workspace : runtime.workspace?.path || '本地工作区';
  }
  if (models) state.models = array(models, 'models').filter((m) => m && m.id);
  if (sessions) state.sessions = array(sessions, 'sessions').filter((s) => s && (s.id || typeof s === 'string'));
  if (objects) state.objects = array(objects, 'objects');
  if (skills) state.skills = array(skills, 'skills');
  if (ui && typeof ui === 'object') state.ui = { ...state.ui, ...ui };
}

// 会话列表的两层稳定排序（ZCode taskListOrdering 移植）：
// running 层置顶且按 createdAt + id 决胜（禁用 updatedAt，防流式事件抖动），
// 其余按 updated_at 倒序，归档沉底。
export function sortedSessions(sessions = state.sessions) {
  const key = (s) => (typeof s === 'string' ? { id: s } : s);
  return sessions.map(key).slice().sort((a, b) => {
    const archA = a.archived ? 1 : 0;
    const archB = b.archived ? 1 : 0;
    if (archA !== archB) return archA - archB;
    const runA = state.runningSessions.has(a.id) ? 1 : 0;
    const runB = state.runningSessions.has(b.id) ? 1 : 0;
    if (runA !== runB) return runB - runA;
    if (runA) {
      const metaA = state.runningSessions.get(a.id);
      const metaB = state.runningSessions.get(b.id);
      if (metaA.startedAt !== metaB.startedAt) return metaB.startedAt - metaA.startedAt;
      return String(a.id).localeCompare(String(b.id));
    }
    return Number(b.updated_at || 0) - Number(a.updated_at || 0);
  });
}

// 今天 / 昨天 / 更早 的本地日历日分组（OpenCode groupSessions 移植）。
export function groupSessions(sessions) {
  const startOfDay = (date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const today = startOfDay(new Date());
  const yesterday = today - 86_400_000;
  const groups = [
    { id: 'today', title: '今天', sessions: [] },
    { id: 'yesterday', title: '昨天', sessions: [] },
    { id: 'older', title: '更早', sessions: [] },
  ];
  for (const session of sessions) {
    const stamp = Number(session.updated_at || session.created_at || 0);
    if (stamp >= today) groups[0].sessions.push(session);
    else if (stamp >= yesterday) groups[1].sessions.push(session);
    else groups[2].sessions.push(session);
  }
  return groups.filter((g) => g.sessions.length);
}
