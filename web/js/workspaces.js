// 工作区注册表：全局列表（不受当前 /w/{id} 前缀影响）+ 新建 / 重命名 / 摘除。
// managed=false 的条目（default 与配置声明）只读，服务端会拒绝变更。

import { apiRoot, browseList, browseMkdir } from './api.js';
import { h, button, openDialog, toast } from './ui.js';

export function listWorkspaces() {
  return apiRoot('/api/workspaces');
}

export function createWorkspace({ path, name }) {
  return apiRoot('/api/workspaces', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path, name: name || undefined }),
  });
}

export function renameWorkspace(id, name) {
  return apiRoot(`/api/workspaces/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  });
}

export function removeWorkspace(id) {
  return apiRoot(`/api/workspaces/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

// 拼接子目录：兼容 "D:\" 与 "D:/parent" 两种基路径。
function joinPath(base, name) {
  if (!base) return name;
  if (/[\\/]$/.test(base)) return base + name;
  return base + (base.includes('\\') ? '\\' : '/') + name;
}

// 资源管理器式目录选择：进入/上级导航 + 就地新建文件夹。
// 返回 { el, getPath }；初始为根视图（Windows 列盘符），onChange 随导航回报当前目录。
function directoryPicker({ onChange } = {}) {
  let current = { path: '', parent: null };
  let loading = false;
  const pathLabel = h('code', { class: 'ws-picker-path mono', 'aria-live': 'polite' }, '正在加载…');
  const upBtn = button('↑ 上级', {
    'data-variant': 'ghost', 'data-size': 'sm', disabled: true,
    onclick: () => load(current.parent ?? ''),
    title: '返回上级目录',
  });
  const listEl = h('div', { class: 'ws-picker', role: 'listbox', 'aria-label': '设备目录' });

  async function load(path) {
    if (loading) return;
    loading = true;
    try {
      render(await browseList(path));
    } catch (error) {
      toast(`无法打开目录：${error.message}`, 'error');
    } finally {
      loading = false;
    }
  }

  function render(data) {
    current = data;
    pathLabel.textContent = data.path || '此设备 · 选择盘符';
    upBtn.disabled = !data.parent;
    onChange?.(data.path);
    listEl.replaceChildren();
    if (!data.entries.length) {
      listEl.append(h('p', { class: 'ws-picker-empty' }, '此目录没有子目录。'));
      return;
    }
    for (const entry of data.entries) {
      listEl.append(h('button', {
        type: 'button', class: 'ws-picker-row', role: 'option', 'aria-label': `进入 ${entry.name}`,
        onclick: () => load(entry.path),
      },
        h('span', { class: 'ws-picker-icon', 'aria-hidden': 'true' }, '📁'),
        h('span', { class: 'ws-picker-name mono' }, entry.name)));
    }
  }

  const newFolderInput = h('input', {
    'data-component': 'input', placeholder: '文件夹名称', maxlength: '120',
    'aria-label': '新建文件夹名称',
  });
  const newFolderBtn = button('＋ 新建文件夹', {
    'data-variant': 'ghost', 'data-size': 'sm',
    onclick: async () => {
      const name = newFolderInput.value.trim();
      if (!name) { newFolderInput.setAttribute('aria-invalid', 'true'); newFolderInput.focus(); return; }
      if (!current.path) { toast('请先进入一个具体目录', 'error'); return; }
      try {
        const created = await browseMkdir(joinPath(current.path, name));
        newFolderInput.value = '';
        toast(`已创建文件夹：${created.path}`);
        await load(current.path);
      } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
    },
  });

  const el = h('div', { class: 'flex flex-col gap-2' },
    h('div', { class: 'ws-picker-bar' }, upBtn, pathLabel),
    listEl,
    h('div', { class: 'ws-picker-actions' },
      newFolderInput, newFolderBtn,
      h('span', { class: 'text-ui-xs text-subtle' }, '在当前目录下新建')));

  load('');
  return { el, getPath: () => current.path };
}

export function newWorkspaceDialog({ onCreated }) {
  const picker = directoryPicker({
    onChange: (path) => { createBtn.disabled = !path; },
  });
  const nameInput = h('input', {
    'data-component': 'input', maxlength: '64',
    placeholder: '留空则取目录名', 'aria-label': '显示名称',
  });
  const row = (label, el) => h('label', { class: 'flex flex-col gap-1' },
    h('span', { 'data-component': 'label' }, label), el);
  const createBtn = button('创建工作区', {
    'data-variant': 'default',
    disabled: true,
    onclick: async () => {
      const path = picker.getPath();
      if (!path) return;
      try {
        const created = await createWorkspace({ path, name: nameInput.value.trim() });
        dialog.close();
        toast(`已创建工作区：${created.name}`);
        onCreated?.(created);
      } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
    },
  });
  const dialog = openDialog({
    title: '新建工作区',
    description: '像资源管理器一样浏览设备目录并选中；也可以就地新建文件夹。运行时会为该路径创建独立运行时与数据目录。',
    body: h('div', { class: 'flex flex-col gap-3' },
      row('设备目录', picker.el),
      row('显示名称', nameInput)),
    footer: [
      button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
      createBtn,
    ],
  });
  nameInput.focus();
}

export function renameWorkspaceDialog(record, { onChanged }) {
  const input = h('input', { 'data-component': 'input', value: record.name, maxlength: '64', 'aria-label': '工作区名称' });
  const dialog = openDialog({
    title: '重命名工作区',
    body: h('div', { class: 'flex flex-col gap-2' }, input),
    footer: [
      button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
      button('保存', {
        'data-variant': 'default',
        onclick: async () => {
          const name = input.value.trim();
          if (!name) { input.setAttribute('aria-invalid', 'true'); return; }
          try {
            await renameWorkspace(record.id, name);
            dialog.close();
            toast('已重命名工作区');
            onChanged?.();
          } catch (error) { toast(`重命名失败：${error.message}`, 'error'); }
        },
      }),
    ],
  });
  input.select();
}

export function removeWorkspaceDialog(record, { onRemoved }) {
  const dialog = openDialog({
    title: '摘除工作区',
    description: `“${record.name}”将从本服务摘除并停止其运行时；磁盘上的会话与数据目录保留，重启后不再自动挂载。`,
    footer: [
      button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
      button('摘除', {
        'data-variant': 'destructive',
        onclick: async () => {
          try {
            await removeWorkspace(record.id);
            dialog.close();
            toast('已摘除工作区');
            onRemoved?.();
          } catch (error) { toast(`摘除失败：${error.message}`, 'error'); }
        },
      }),
    ],
  });
}

// 从 location 推导当前工作区 id（'' → default）。
export function workspaceIdFromLocation() {
  const match = location.pathname.match(/^\/w\/([^/]+)/);
  return match ? decodeURIComponent(match[1]) : 'default';
}
