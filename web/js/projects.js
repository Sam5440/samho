// 项目分类管理：会话按项目归类的宿主。
// 提供侧栏过滤 chips、行内「移动到项目」菜单与项目管理对话框（增删改/配色）。
// 项目是轻量元数据（id/name/color），归属关系存在会话元数据 project_id 上。

import { h, cn, button, openMenu, openDialog, toast } from './ui.js';
import { api, apiPost, apiPatch, apiDelete } from './api.js';
import { state } from './store.js';

// 新项目默认色轮换（与设计 token 同源的基色值，属于数据而非主题样式）。
const DEFAULT_COLORS = ['#7fae32', '#3b82f6', '#0d9488', '#db2777', '#f59e0b', '#7c3aed'];
let colorCursor = 0;

export function projectById(id) {
  return state.projects.find((p) => p && p.id === id) || null;
}

export async function refreshProjects() {
  try {
    state.projects = await api('/api/projects');
  } catch (_) { /* 连接错误由顶栏状态指示 */ }
  return state.projects;
}

/* ---------- 侧栏过滤 chips：全部 / 各项目 / 未分类 ---------- */
export function createProjectFilter({ onChange }) {
  let value = 'all';
  const el = h('div', { class: 'project-chips', role: 'group', 'aria-label': '按项目过滤会话' });

  function chip(label, key, color) {
    const active = value === key;
    const btn = h('button', {
      type: 'button',
      class: cn('project-chip', active && 'is-active'),
      'aria-pressed': String(active),
    },
      color ? h('span', { class: 'project-chip-dot', style: { background: color } }) : null,
      label);
    btn.addEventListener('click', () => {
      if (value === key) return;
      value = key;
      refresh();
      onChange(value);
    });
    return btn;
  }

  function refresh() {
    el.replaceChildren();
    if (!state.projects.length) return;
    el.append(chip('全部', 'all'));
    for (const project of state.projects) el.append(chip(project.name, project.id, project.color));
    el.append(chip('未分类', 'none'));
  }

  refresh();
  return { el, refresh };
}

/* ---------- 移动到项目 ---------- */
async function assignProject(sessionId, projectId) {
  try {
    await apiPatch(`/api/sessions/${encodeURIComponent(sessionId)}`, { project_id: projectId });
    toast(projectId ? '已移动到项目' : '已移出项目');
    return true;
  } catch (error) {
    toast(`移动失败：${error.message}`, 'error');
    return false;
  }
}

export function moveToProjectMenu(session, anchor, onDone) {
  const items = state.projects.map((project) => ({
    label: project.name,
    icon: h('span', { class: 'project-chip-dot', style: { background: project.color || 'var(--border-strong)' } }),
    onSelect: async () => { if (await assignProject(session.id, project.id)) onDone?.(); },
  }));
  if (session.project_id) {
    items.push({
      label: '移出项目',
      icon: h('span', { class: 'project-chip-dot', style: { background: 'transparent', border: '1px dashed var(--border-strong)' } }),
      onSelect: async () => { if (await assignProject(session.id, null)) onDone?.(); },
    });
  }
  if (!items.length) {
    toast('还没有项目，先在「管理项目」里创建');
    return;
  }
  openMenu(anchor, items, { align: 'start' });
}

/* ---------- 管理项目对话框 ---------- */
export function manageProjectsDialog({ onChanged }) {
  const listEl = h('div', { class: 'flex flex-col gap-2' });
  const nameInput = h('input', {
    'data-component': 'input',
    placeholder: '新项目名称…',
    maxlength: '64',
    'aria-label': '项目名称',
  });

  function swatch(project, color) {
    const dot = h('button', {
      type: 'button',
      class: 'project-swatch',
      style: { background: color },
      'aria-label': `把「${project.name}」设为该颜色`,
      title: '更换颜色',
    });
    if ((project.color || '').toLowerCase() === color) dot.setAttribute('aria-pressed', 'true');
    dot.addEventListener('click', async () => {
      try {
        await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { color });
        await refresh();
        onChanged?.();
      } catch (error) { toast(`颜色保存失败：${error.message}`, 'error'); }
    });
    return dot;
  }

  async function refresh() {
    await refreshProjects();
    listEl.replaceChildren();
    if (!state.projects.length) {
      listEl.append(h('p', { class: 'text-ui-sm text-muted' }, '还没有项目。创建一个，然后把会话移动进去分类管理。'));
    }
    for (const project of state.projects) {
      const count = state.sessions.filter((s) => s && s.project_id === project.id).length;
      const name = h('button', {
        type: 'button',
        class: 'project-manage-name truncate',
        title: '点击重命名',
      }, project.name);
      name.addEventListener('click', () => renameDialog(project));
      listEl.append(h('div', { class: 'project-manage-row' },
        h('span', { class: 'project-chip-dot', style: { background: project.color || 'var(--border-strong)' } }),
        name,
        h('span', { class: 'project-manage-count mono' }, `${count} 会话`),
        h('span', { class: 'project-colors' }, DEFAULT_COLORS.map((color) => swatch(project, color))),
        button('删除…', {
          'data-variant': 'ghost', 'data-size': 'sm',
          onclick: () => deleteDialog(project),
        }),
      ));
    }
  }

  function renameDialog(project) {
    const input = h('input', { 'data-component': 'input', value: project.name, maxlength: '64', 'aria-label': '项目名称' });
    const dialog = openDialog({
      title: '重命名项目',
      body: h('div', { class: 'flex flex-col gap-2' }, input),
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('保存', {
          'data-variant': 'default',
          onclick: async () => {
            const name = input.value.trim();
            if (!name) { input.setAttribute('aria-invalid', 'true'); return; }
            try {
              await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { name });
              dialog.close();
              await refresh();
              onChanged?.();
              toast('已重命名项目');
            } catch (error) { toast(`重命名失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  function deleteDialog(project) {
    const count = state.sessions.filter((s) => s && s.project_id === project.id).length;
    const dialog = openDialog({
      title: '删除项目',
      description: `项目“${project.name}”将被删除，${count} 个会话回到未分类。会话本身与执行轨迹不受影响。`,
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('删除项目', {
          'data-variant': 'destructive',
          onclick: async () => {
            try {
              await apiDelete(`/api/projects/${encodeURIComponent(project.id)}`);
              dialog.close();
              await refresh();
              onChanged?.();
              toast('已删除项目，相关会话回到未分类');
            } catch (error) { toast(`删除失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  async function create() {
    const name = nameInput.value.trim();
    if (!name) { nameInput.setAttribute('aria-invalid', 'true'); return; }
    try {
      await apiPost('/api/projects', { name, color: DEFAULT_COLORS[colorCursor % DEFAULT_COLORS.length] });
      colorCursor += 1;
      nameInput.value = '';
      await refresh();
      onChanged?.();
      toast('已创建项目');
    } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
  }

  const dialog = openDialog({
    title: '管理项目',
    description: '项目用来给会话分类。删除项目不会删除会话。',
    size: 'lg',
    body: h('div', { class: 'flex flex-col gap-3' },
      listEl,
      h('div', { class: 'flex gap-2' }, nameInput, button('创建项目', { 'data-variant': 'default', onclick: create })),
    ),
  });
  nameInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); create(); }
  });
  refresh();
  return dialog;
}
