// 工作区侧栏：工作区列表（路径指定，动态增删/重命名），
// 活动工作区展开其会话侧栏（createSessionSidebar，随前缀自动读写当前工作区）。
// main.js 通过 onSwitchWorkspace 完成前缀切换与数据重载。

import { h, cn, button, openMenu, toast } from './ui.js';
import { createSessionSidebar } from './sessions.js';
import {
  listWorkspaces, newWorkspaceDialog, renameWorkspaceDialog, removeWorkspaceDialog,
  workspaceIdFromLocation,
} from './workspaces.js';

export function createWorkbenchSidebar({ container, onSelect, onNewSession, onSwitchWorkspace }) {
  let records = [];
  // 直接以 /w/{id} 打开页面时，初始活动工作区取自 URL。
  let currentWs = workspaceIdFromLocation();
  let expanded = new Set([currentWs]);

  const list = h('nav', { class: 'ws-list min-h-0', 'aria-label': '工作区列表' });
  const chatPane = h('div', { class: 'sidebar-pane min-h-0', role: 'tabpanel', 'aria-label': '当前工作区会话' });
  const chat = createSessionSidebar({ container: chatPane, onSelect, onNewSession });
  const addBtn = button('＋ 新建工作区', {
    'data-variant': 'default', class: 'session-new',
    // 创建成功后走统一切换流（前缀/pushState/数据重载都由 main.js 驱动）。
    onclick: () => newWorkspaceDialog({ onCreated: (created) => onSwitchWorkspace?.(created.id) }),
  });

  function wsMenu(record, anchor) {
    const items = [
      { label: '切换到此工作区', icon: '↗', onSelect: () => onSwitchWorkspace?.(record.id) },
      { label: '重命名…', onSelect: () => renameWorkspaceDialog(record, { onChanged: () => reload() }) },
    ];
    if (record.managed) {
      items.push('-');
      items.push({
        label: '摘除工作区…', variant: 'destructive',
        // 摘除的是当前工作区时回到 default（前缀切换同样走统一流程）。
        onSelect: () => removeWorkspaceDialog(record, {
          onRemoved: () => {
            if (record.id === currentWs) onSwitchWorkspace?.('default');
            else reload();
          },
        }),
      });
    }
    openMenu(anchor, items, { align: 'end' });
  }

  function wsRow(record) {
    const active = record.id === currentWs;
    const open = active && expanded.has(record.id);
    const rowEl = h('div', { class: cn('ws-row', active && 'is-active') });
    const main = h('button', {
      type: 'button', class: 'ws-row-main',
      'aria-current': active ? 'page' : 'false',
      'aria-expanded': active ? String(open) : undefined,
      title: record.path,
      onclick: () => {
        if (active) {
          expanded.has(record.id) ? expanded.delete(record.id) : expanded.add(record.id);
          render();
          return;
        }
        onSwitchWorkspace?.(record.id);
      },
    },
      active ? h('span', { class: 'ws-chevron text-subtle', 'aria-hidden': 'true' }, open ? '▾' : '▸') : h('span', { class: 'ws-chevron', 'aria-hidden': 'true' }),
      h('span', { class: 'ws-copy min-w-0' },
        h('span', { class: 'ws-name' }, record.name || record.id),
        h('span', { class: 'ws-path mono' }, record.path || '/')),
      record.managed ? h('span', { class: 'ws-badge', title: '动态挂载，可摘除' }, '托管') : null,
    );
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm', class: 'ws-menu', 'aria-label': '工作区操作',
      onclick: (event) => { event.stopPropagation(); wsMenu(record, event.currentTarget); },
    });
    rowEl.append(main, menuBtn);
    if (active && open) rowEl.append(h('div', { class: 'ws-children' }, chatPane));
    return rowEl;
  }

  function render() {
    list.replaceChildren();
    if (!records.length) {
      list.append(h('p', { class: 'session-empty' }, '正在读取工作区…'));
      return;
    }
    for (const record of records) list.append(wsRow(record));
  }

  async function reload(switchTo, { force = false } = {}) {
    try {
      records = await listWorkspaces();
    } catch (_) {
      list.replaceChildren(h('p', { class: 'session-empty' }, '无法读取工作区列表。'));
      return;
    }
    if (switchTo && records.some((r) => r.id === switchTo) && (switchTo !== currentWs || force)) {
      currentWs = switchTo;
      expanded = new Set([switchTo, ...expanded]);
    }
    if (!records.some((r) => r.id === currentWs)) currentWs = records[0]?.id || 'default';
    render();
  }

  container.replaceChildren(addBtn, list);
  render();
  reload();

  return {
    async refresh() {
      await chat.refresh();
    },
    async reloadWorkspaces(switchTo) {
      await reload(switchTo);
    },
    setCurrent(id) { chat.setCurrent(id); },
    get currentId() { return chat.currentId; },
    get currentWorkspace() { return currentWs; },
  };
}
