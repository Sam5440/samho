// 任务看板：当前工作区任务按状态分组（运行中 / 已完成 / 失败 / 已取消），
// 数据 GET /api/tasks；运行中任务可停止。与子代理面板同源，视角为任务生命周期。

import { h, badge, button, spinner, toast } from '../ui.js';
import { api, apiPost } from '../api.js';

const COLUMNS = [
  { key: 'running', label: '运行中', variant: 'info' },
  { key: 'completed', label: '已完成', variant: 'success' },
  { key: 'failed', label: '失败', variant: 'destructive' },
  { key: 'cancelled', label: '已取消', variant: 'secondary' },
];

function taskCard(task) {
  const progress = task.progress || {};
  const card = h('article', { class: 'task-card', 'data-status': task.status });
  const title = task.result && typeof task.result === 'object'
    ? String(task.result.output || task.result.session_id || '').slice(0, 80)
    : '';
  const head = h('div', { class: 'task-card-head' },
    h('code', { class: 'mono text-ui-xs' }, String(task.task_id).slice(0, 8)),
    task.status === 'running' ? spinner('0.7rem') : null,
    progress.model_id ? h('span', { class: 'text-ui-xs text-subtle' }, progress.model_id) : null);
  card.append(head);
  if (title) card.append(h('p', { class: 'task-card-title' }, title));
  card.append(h('div', { class: 'task-card-meta' },
    progress.turn ? h('span', {}, `轮次 ${progress.turn}`) : null,
    progress.usage ? h('span', {}, `tokens ${progress.usage.total_tokens ?? '–'}`) : null));
  if (task.status === 'running') {
    card.append(h('div', { class: 'task-card-actions' }, button('停止任务', {
      'data-variant': 'outline', 'data-size': 'sm',
      onclick: async () => {
        try {
          await apiPost(`/api/tasks/${encodeURIComponent(task.task_id)}/cancel`, {});
          toast('已请求停止任务');
          refreshInto(card.closest('.task-board'));
        } catch (error) { toast(`停止失败：${error.message}`, 'error'); }
      },
    })));
  }
  return card;
}

async function refreshInto(container) {
  let tasks = [];
  try {
    const body = await api('/api/tasks');
    tasks = Array.isArray(body?.tasks) ? body.tasks : [];
  } catch (error) {
    container.replaceChildren(h('p', { class: 'text-ui-sm text-muted' }, `加载失败：${error.message}`));
    return;
  }
  const columns = COLUMNS.map((column) => {
    const list = tasks.filter((task) => task.status === column.key);
    return h('div', { class: 'task-column', 'data-status': column.key },
      h('div', { class: 'task-column-head' },
        h('span', { class: 'task-column-label' }, column.label),
        badge(String(list.length), column.variant)),
      list.length
        ? h('div', { class: 'task-column-body' }, ...list.map(taskCard))
        : h('p', { class: 'task-column-empty' }, '—'));
  });
  container.replaceChildren(
    tasks.length
      ? h('div', { class: 'task-board' }, ...columns)
      : h('p', { class: 'text-ui-sm text-subtle' }, '还没有任务记录；发送一条消息即会产生任务。'));
}

export function createTasksPanel({ container }) {
  let timer = 0;
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
