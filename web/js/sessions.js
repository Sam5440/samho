// 会话侧栏：运行中置顶稳定排序 + 今天/昨天/更早分组 + 归档分区 + 搜索
// + 行内操作（重命名 / 归档 / 分支 / 删除两击确认）。

import { h, cn, button, badge, spinner, openMenu, openDialog, toast, bindTooltip, relativeTime } from './ui.js';
import { api, apiPatch, apiPost, apiDelete } from './api.js';
import { state, sortedSessions, groupSessions, on, emit } from './store.js';
const titleText = (session) => String(session?.title || session?.id || '未命名会话');

function statusDot(session, running) {
  if (running) return h('span', { class: 'session-dot is-loading' }, spinner('0.8rem'));
  const s = String(session?.status || 'ready');
  if (s === 'failed' || s === 'error') return h('span', { class: 'session-dot is-error', title: '上次执行失败' });
  if (s === 'interrupted') return h('span', { class: 'session-dot is-interrupted', title: '上次被中断' });
  return h('span', { class: 'session-dot is-idle' });
}

export function createSessionSidebar({ container, onSelect, onNewSession }) {
  let filter = '';
  let currentId = state.currentSessionId;
  let archiveOpen = false;

  const searchInput = h('input', {
    'data-component': 'input',
    placeholder: '搜索会话…',
    spellcheck: 'false',
    oninput: () => { filter = searchInput.value.trim().toLowerCase(); render(); },
  });
  const list = h('nav', { class: 'session-list min-h-0', 'aria-label': '历史会话' });
  const newButton = button('＋ 新建任务', { 'data-variant': 'default', class: 'session-new', onclick: () => onNewSession?.() });

  function rowMenu(session, anchor) {
    const archived = Boolean(session.archived);
    openMenu(anchor, [
      { label: '打开会话', icon: '↗', onSelect: () => onSelect(session.id) },
      '-',
      { label: '重命名', onSelect: () => renameDialog(session) },
      { label: '分支会话', onSelect: () => forkSession(session) },
      '-',
      { label: archived ? '取消归档' : '归档', onSelect: () => setArchived(session, !archived) },
      { label: '删除会话…', variant: 'destructive', onSelect: () => deleteDialog(session) },
    ], { align: 'end' });
  }

  async function renameDialog(session) {
    const input = h('input', { 'data-component': 'input', value: titleText(session), maxlength: '200', 'aria-label': '会话标题' });
    const dialog = openDialog({
      title: '重命名会话',
      body: h('div', { class: 'flex flex-col gap-2' }, input),
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('保存', {
          'data-variant': 'default',
          onclick: async () => {
            const title = input.value.trim();
            if (!title) { input.setAttribute('aria-invalid', 'true'); return; }
            try {
              await apiPatch(`/api/sessions/${encodeURIComponent(session.id)}`, { title });
              dialog.close();
              await refresh();
              toast('已重命名会话');
              emit('session-renamed', { id: session.id, title });
            } catch (error) { toast(`重命名失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  async function setArchived(session, archived) {
    try {
      await apiPatch(`/api/sessions/${encodeURIComponent(session.id)}`, { archived });
      session.archived = archived;
      await refresh();
      toast(archived ? '已归档，可在归档区找到' : '已取消归档');
    } catch (error) { toast(`归档操作失败：${error.message}`, 'error'); }
  }

  function deleteDialog(session) {
    const dialog = openDialog({
      title: '删除会话',
      description: `“${titleText(session)}”及其所有分支将被永久删除，执行轨迹保留在事件日志中。`,
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('永久删除', {
          'data-variant': 'destructive',
          onclick: async () => {
            try {
              await apiDelete(`/api/sessions/${encodeURIComponent(session.id)}`);
              dialog.close();
              if (currentId === session.id) onSelect(null, { forceNew: true });
              await refresh();
              toast('会话已删除');
            } catch (error) { toast(`删除失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  async function forkSession(session) {
    try {
      const fork = await apiPost(`/api/sessions/${encodeURIComponent(session.id)}/fork`, {});
      await refresh();
      toast(`已创建分支：${titleText(fork)}`);
      onSelect(fork.id);
    } catch (error) { toast(`分支失败：${error.message}`, 'error'); }
  }

  function sessionRow(session) {
    const id = session.id;
    const running = state.runningSessions.has(id);
    const selected = id === currentId;
    const row = h('div', { class: cn('session-row', selected && 'is-selected') });
    const main = h('button', {
      type: 'button',
      class: 'session-row-main',
      'aria-current': selected ? 'page' : 'false',
      onclick: () => onSelect(id),
    },
      statusDot(session, running),
      h('span', { class: 'session-copy min-w-0' },
        h('span', { class: 'session-title' }, titleText(session)),
        h('span', { class: 'session-meta' },
          session.last_model ? h('span', { class: 'mono' }, session.last_model) : null,
          session.parent_id ? badge('分支', 'outline') : null,
          h('span', { class: 'text-subtle' }, relativeTime(session.updated_at)),
        ),
      ),
    );
    bindTooltip(main.querySelector('.session-title'), titleText(session));
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm',
      class: 'session-menu', 'aria-label': '会话操作',
      onclick: (event) => { event.stopPropagation(); rowMenu(session, event.currentTarget); },
    });
    row.append(main, menuBtn);
    return row;
  }

  function sectionLabel(title, count) {
    return h('div', { class: 'session-group-label' },
      h('span', {}, title),
      count !== undefined ? h('span', { class: 'count' }, String(count)) : null);
  }

  function render() {
    list.replaceChildren();
    const all = sortedSessions();
    const filtered = filter
      ? all.filter((s) => titleText(s).toLowerCase().includes(filter))
      : all;
    const active = filtered.filter((s) => !s.archived);
    const archived = filtered.filter((s) => s.archived);
    if (!active.length && !archived.length) {
      list.append(h('p', { class: 'session-empty' }, filter ? '没有匹配的会话。' : '还没有会话，从新任务开始。'));
      return;
    }
    for (const group of groupSessions(active)) {
      list.append(sectionLabel(group.title, group.sessions.length));
      for (const session of group.sessions) list.append(sessionRow(session));
    }
    if (archived.length) {
      const head = h('button', {
        type: 'button', class: 'session-group-label is-toggle',
        'aria-expanded': String(archiveOpen),
        onclick: () => { archiveOpen = !archiveOpen; render(); },
      },
        h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, archiveOpen ? '▾' : '▸'),
        h('span', {}, `归档`),
        h('span', { class: 'count' }, String(archived.length)));
      list.append(head);
      if (archiveOpen) for (const session of archived) list.append(sessionRow(session));
    }
  }

  async function refresh() {
    try {
      state.sessions = await api('/api/sessions');
      render();
    } catch (_) { /* 连接错误由顶栏状态指示 */ }
  }

  on('sessions-changed', render);
  on('running-changed', render);

  container.append(newButton, h('div', { class: 'session-search' }, searchInput), list);
  render();

  return {
    refresh,
    setCurrent(id) { currentId = id; render(); },
    get currentId() { return currentId; },
  };
}
