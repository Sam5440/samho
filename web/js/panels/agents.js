// 子代理面板：运行中的智能体执行（每任务一个 agent loop）。
// 来源 GET /api/tasks：running 快照附实时进度（模型 / 轮次 / 工具活动）。

import { h, badge, button, spinner, toast } from '../ui.js';
import { api, apiPost } from '../api.js';

const STATUS_LABELS = {
  running: '执行中',
  completed: '已完成',
  done: '已完成',
  failed: '失败',
  cancelled: '已取消',
};

function agentCard(task) {
  const progress = task.progress || {};
  const running = task.status === 'running';
  const statusKey = running ? 'running' : task.status;
  const modelId = progress.model_id || task.result?.model_id;
  const card = h('article', { class: 'agent-card', 'data-status': statusKey });
  const head = h('div', { class: 'agent-card-head' },
    h('span', { class: 'agent-card-title' },
      running ? spinner('0.75rem') : h('span', { class: 'agent-dot', 'aria-hidden': 'true' }),
      modelId ? h('code', { class: 'mono text-ui-xs' }, modelId) : '代理'),
    badge(STATUS_LABELS[statusKey] || task.status,
      running ? 'info' : task.status === 'failed' ? 'destructive' : 'secondary'));
  const turnLabel = running
    ? `轮次 ${progress.turn ?? '–'} · 尝试 ${progress.attempt ?? 1}`
    : `轮次 ${task.result?.turns ?? '–'} · $${Number(task.result?.cost_usd ?? 0).toFixed(4)}`;
  const meta = h('div', { class: 'agent-card-meta' },
    h('span', {}, turnLabel),
    progress.trace_id ? h('code', { class: 'mono text-ui-xs text-subtle' }, progress.trace_id.slice(0, 8)) : null);
  const live = running && (progress.text || '').trim()
    ? h('p', { class: 'agent-card-live' }, String(progress.text).slice(-160))
    : null;
  const failed = task.status === 'failed' && task.error
    ? h('p', { class: 'agent-card-error' }, String(task.error).slice(0, 200))
    : null;
  const actions = [];
  if (running && progress.session_id) {
    actions.push(button('打开会话', {
      'data-variant': 'ghost', 'data-size': 'sm',
      onclick: () => {
        window.dispatchEvent(new CustomEvent('agents-open-session', {
          detail: { sessionId: progress.session_id },
        }));
      },
    }));
  }
  if (running) {
    actions.push(button('停止', {
      'data-variant': 'ghost', 'data-size': 'sm', class: 'agent-card-stop',
      onclick: async () => {
        try {
          await apiPost(`/api/tasks/${encodeURIComponent(task.task_id)}/cancel`, {});
          toast('已请求停止');
          refresh();
        } catch (error) { toast(`停止失败：${error.message}`, 'error'); }
      },
    }));
  }
  card.append(head, meta);
  if (live) card.append(live);
  if (failed) card.append(failed);
  if (actions.length) card.append(h('div', { class: 'agent-card-actions' }, ...actions));
  return card;
}

let timer = 0;

async function refreshInto(container) {
  let tasks = [];
  try {
    const body = await api('/api/tasks');
    tasks = Array.isArray(body?.tasks) ? body.tasks : [];
  } catch (error) {
    container.replaceChildren(h('p', { class: 'text-ui-sm text-muted' }, `加载失败：${error.message}`));
    return;
  }
  const running = tasks.filter((task) => task.status === 'running');
  const finished = tasks.filter((task) => task.status !== 'running').slice(0, 12);
  const section = (label, list, emptyText) => h('div', { class: 'agents-section' },
    h('div', { class: 'agents-section-label' }, label, badge(String(list.length), 'secondary')),
    list.length
      ? h('div', { class: 'agents-list' }, ...list.map(agentCard))
      : h('p', { class: 'text-ui-sm text-subtle' }, emptyText));
  container.replaceChildren(
    section('运行中', running, '没有正在执行的子代理。'),
    section('最近完成', finished, '暂无历史执行。'));
}

export function createAgentsPanel({ container }) {
  return {
    show() {
      clearInterval(timer);
      refreshInto(container);
      timer = setInterval(() => refreshInto(container), 2500);
      return Promise.resolve();
    },
    hide() { clearInterval(timer); timer = 0; },
    refresh: () => refreshInto(container),
  };
}
