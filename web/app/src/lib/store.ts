// 全局状态（zustand）。只放需要驱动渲染的数据；竞态守卫、定时器等过程状态在 controller.ts。

import { create } from 'zustand';
import type { LiveTool, Message, TraceEvent } from './projection';

/* eslint-disable @typescript-eslint/no-explicit-any */
export type Session = {
  id: string;
  title?: string;
  status?: string;
  archived?: boolean;
  project_id?: string | null;
  parent_id?: string | null;
  last_model?: string | null;
  updated_at?: number;
  created_at?: number;
};

export type Project = {
  id: string;
  name: string;
  color?: string | null;
  created_at?: string | number;
  folders?: string[];
  entry?: string | null;
  open?: boolean;
};

export type Model = {
  id: string;
  tier?: string;
  provider?: string;
  context_window?: number;
  [key: string]: unknown;
};

export type Tier = { id: string; name?: string; label?: string; rank?: number; min_complexity?: number; max_complexity?: number };

export type WorkspaceRecord = { id: string; name: string; path?: string; managed?: boolean };

export type UiSettings = { theme: 'dark' | 'light' | 'system'; submit_key: string; new_session_key: string; cancel_key: string };

export type Task = {
  id: string;
  sessionId: string | null;
  viewKey: string;
  prompt: string;
  startedAt: number;
  progress: any | null;
  cancelRequested: boolean;
};

export type DeviceTab = 'files' | 'git' | 'term' | 'browser';
export type InspectTab = 'trajectory' | 'agents' | 'tasks' | 'objects' | 'mcp' | 'skills' | 'models';
export type PanelName = DeviceTab | InspectTab;

export const DEVICE_TABS: DeviceTab[] = ['files', 'git', 'term', 'browser'];
export const INSPECT_TABS: InspectTab[] = ['trajectory', 'agents', 'tasks', 'objects', 'mcp', 'skills', 'models'];

export const tierNames: Record<string, string> = {
  nano: '轻量', fast: '快速', balanced: '均衡',
  reasoning: '推理', frontier: '前沿', specialist: '专家',
};

export type State = {
  apiOnline: boolean | null;
  simulation: boolean;
  workspaceLabel: string;
  workspacePath: string;
  models: Model[];
  tiers: Tier[];
  sessions: Session[];
  projects: Project[];
  objects: any[];
  skills: any[];
  mcp: any | null;
  limits: { max_turns?: number } | null;
  ui: UiSettings;
  lanUrls: string[];

  workspaces: WorkspaceRecord[];
  currentWorkspaceId: string;

  currentSessionId: string | null;
  draftKey: string;
  transcripts: Record<string, Message[]>;

  task: Task | null;
  runningSessions: Record<string, { taskId: string; startedAt: number }>;
  liveTools: Record<string, LiveTool>;

  selectedSkills: string[];
  request: { tier: string; model: string; automatic: boolean; complexity: number };

  traceId: string;
  trace: TraceEvent[];

  streamStatus: string;
  activePanel: PanelName;
  lastByGroup: { device: DeviceTab; inspect: InspectTab };
  inspectorOpen: boolean;
  sidebarOpen: boolean;
  modelPickerOpen: boolean;
  settingsOpen: boolean;
  announcement: string;
  // 附属面板（终端/浏览器）绑定数变化计数：驱动侧栏附属行重绘。
  attachVersion: number;
};

let draftCounter = 0;
export const nextDraftKey = () => `draft:${++draftCounter}`;
export const viewKeyOf = (sessionId: string) => `session:${sessionId}`;

export const useStore = create<State>(() => ({
  apiOnline: null,
  simulation: false,
  workspaceLabel: '',
  workspacePath: '',
  models: [],
  tiers: [],
  sessions: [],
  projects: [],
  objects: [],
  skills: [],
  mcp: null,
  limits: null,
  ui: { theme: 'dark', submit_key: 'Ctrl+Enter', new_session_key: 'Alt+N', cancel_key: 'Ctrl+K' },
  lanUrls: [],

  workspaces: [],
  currentWorkspaceId: 'default',

  currentSessionId: null,
  draftKey: nextDraftKey(),
  transcripts: {},

  task: null,
  runningSessions: {},
  liveTools: {},

  selectedSkills: [],
  request: { tier: '', model: '', automatic: true, complexity: 4 },

  traceId: '',
  trace: [],

  streamStatus: '事件流 / 连接中',
  activePanel: 'trajectory',
  lastByGroup: { device: 'files', inspect: 'trajectory' },
  inspectorOpen: true,
  sidebarOpen: false,
  modelPickerOpen: false,
  settingsOpen: false,
  announcement: '',
  attachVersion: 0,
}));

export const getState = useStore.getState;
export const setState = useStore.setState;

export const currentViewKey = (s: State = getState()) =>
  s.currentSessionId ? viewKeyOf(s.currentSessionId) : s.draftKey;

export function requestSummary(request: State['request']) {
  if (request.model) return request.model;
  if (request.tier) return `${tierNames[request.tier] || request.tier} · 自动选择`;
  return request.automatic ? '自动路由' : `复杂度 ${request.complexity}`;
}

export const sessionTitle = (session: Partial<Session> | null | undefined) =>
  String(session?.title || session?.id || '未命名会话');
