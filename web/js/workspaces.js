// 工作区注册表：全局列表（不受当前 /w/{id} 前缀影响）+ 新建 / 重命名 / 摘除。
// managed=false 的条目（default 与配置声明）只读，服务端会拒绝变更。

import { apiRoot } from './api.js';
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

export function newWorkspaceDialog({ onCreated }) {
  const pathInput = h('input', {
    'data-component': 'input', class: 'mono',
    placeholder: '设备上的绝对路径，如 D:\\projects\\demo', spellcheck: 'false', 'aria-label': '工作区路径',
  });
  const nameInput = h('input', {
    'data-component': 'input', maxlength: '64',
    placeholder: '留空则取目录名', 'aria-label': '显示名称',
  });
  const row = (label, el) => h('label', { class: 'flex flex-col gap-1' },
    h('span', { 'data-component': 'label' }, label), el);
  const dialog = openDialog({
    title: '新建工作区',
    description: '运行时会为该路径创建独立运行时与数据目录，会话互不干扰。',
    body: h('div', { class: 'flex flex-col gap-3' },
      row('路径', pathInput),
      row('显示名称', nameInput)),
    footer: [
      button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
      button('创建', {
        'data-variant': 'default',
        onclick: async () => {
          const path = pathInput.value.trim();
          if (!path) { pathInput.setAttribute('aria-invalid', 'true'); pathInput.focus(); return; }
          try {
            const created = await createWorkspace({ path, name: nameInput.value.trim() });
            dialog.close();
            toast(`已创建工作区：${created.name}`);
            onCreated?.(created);
          } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
        },
      }),
    ],
  });
  pathInput.focus();
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
