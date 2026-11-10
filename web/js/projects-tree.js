// 会话侧栏（项目制）：工作区 → 项目 → 文件夹 + 会话 两层树。
// 项目行展开后列出文件夹（entry 标记主目录）与项目下的会话（保留 fork 树缩进）；
// 会话行附属终端/浏览器默认折叠，展开后可进入/复用绑定面板。
// 项目数据来自 /api/projects（folders/entry/open 字段），会话来自 /api/sessions。

import { h, cn, button, badge, spinner, openMenu, openDialog, toast, bindTooltip, relativeTime } from './ui.js';
import { api, apiPatch, apiPost, apiDelete, apiRoot } from './api.js';
import { state, sortedSessions, groupSessions, on, emit } from './store.js';
import { moveToProjectMenu, projectById, manageProjectsDialog, refreshProjects } from './projects.js';

const titleText = (session) => String(session?.title || session?.id || '未命名会话');

function statusDot(session, running) {
  if (running) return h('span', { class: 'session-dot is-loading' }, spinner('0.8rem'));
  const s = String(session?.status || 'ready');
  if (s === 'failed' || s === 'error') return h('span', { class: 'session-dot is-error', title: '上次执行失败' });
  if (s === 'interrupted') return h('span', { class: 'session-dot is-interrupted', title: '上次被中断' });
  return h('span', { class: 'session-dot is-idle' });
}

/* ---------- 附属面板接口（由 dock 注入） ---------- */
// attachPanel = { openTerminal(sessionId), openBrowser(sessionId), counts(sessionId) }
// counts: { terms: number, tabs: number } 面板当前为该会话绑定的活体数量。

export function createProjectSidebar({ container, onSelect, onNewSession, onChanged }) {
  let filter = '';
  let currentId = state.currentSessionId;
  let archiveOpen = false;
  // 附属行展开状态（session id -> bool；默认折叠）。
  const attachOpenSet = new Set();
  // 项目折叠状态（project id -> bool）；缺省展开。
  const collapsed = new Set();
  // 项目/附属元数据变化时的外部回调（刷新对话时间线标题等）。
  const notifyChanged = typeof onChanged === 'function' ? onChanged : null;

  const searchInput = h('input', {
    'data-component': 'input',
    placeholder: '搜索会话…',
    spellcheck: 'false',
    oninput: () => { filter = searchInput.value.trim().toLowerCase(); render(); },
  });
  const manageBtn = button('管理项目', {
    'data-variant': 'ghost', 'data-size': 'sm',
    onclick: () => manageProjectsDialog({ onChanged: () => refresh() }),
  });
  const list = h('nav', { class: 'session-list min-h-0', 'aria-label': '项目与会话' });
  let attachPanel = null;

  function setAttachPanel(panel) { attachPanel = panel; }

  function projectOpenState(project) {
    return project.open === true;
  }

  async function toggleProject(project) {
    const nextOpen = !projectOpenState(project);
    if (nextOpen) collapsed.delete(project.id);
    else collapsed.add(project.id);
    render();
    try {
      await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { open: nextOpen });
    } catch (_) { /* 折叠是本地优先 UI 状态，持久化失败不阻断 */ }
  }

  /* ---------- 项目行 ---------- */
  function projectRow(project) {
    const open = projectOpenState(project);
    const sessions = state.sessions.filter((s) => s && s.project_id === project.id);
    const row = h('div', { class: cn('project-row', currentId && state.sessions.some((s) => s.id === currentId && s.project_id === project.id) && 'is-active') });
    const main = h('button', {
      type: 'button',
      class: 'project-row-main',
      'aria-expanded': String(open),
      title: project.name,
    },
      h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, open ? '▾' : '▸'),
      h('span', { class: 'project-chip-dot', style: { background: project.color || 'var(--border-strong)' } }),
      h('span', { class: 'project-name min-w-0' }, project.name),
      h('span', { class: 'count mono' }, String(sessions.length)),
    );
    main.addEventListener('click', () => toggleProject(project));
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm',
      class: 'project-menu', 'aria-label': '项目操作',
      onclick: (event) => { event.stopPropagation(); projectMenu(project, event.currentTarget); },
    });
    row.append(main, menuBtn);
    if (open) row.append(projectChildren(project, sessions));
    return row;
  }

  function projectMenu(project, anchor) {
    openMenu(anchor, [
      { label: projectOpenState(project) ? '折叠项目' : '展开项目', onSelect: () => toggleProject(project) },
      { label: '重命名…', onSelect: () => renameProjectDialog(project) },
      { label: '文件夹管理…', onSelect: () => foldersDialog(project) },
      { label: '主目录…', onSelect: () => entryDialog(project) },
      '-',
      { label: '新任务（归属本项目）', icon: '＋', onSelect: () => onNewSession?.(project.id) },
    ], { align: 'start' });
  }

  function projectChildren(project, sessions) {
    const children = h('div', { class: 'project-children' });
    // 文件夹行：entry 一等公民优先。
    const folders = Array.isArray(project.folders) ? project.folders : [];
    for (const folder of folders) {
      const isEntry = project.entry === folder;
      children.append(h('div', { class: 'folder-row' },
        h('span', { class: 'folder-icon', 'aria-hidden': 'true' }, '📁'),
        h('span', { class: 'folder-name mono min-w-0', title: folder }, folder),
        isEntry ? badge('主目录', 'outline', true) : null,
      ));
    }
    children.append(button('＋ 添加文件夹', {
      'data-variant': 'ghost', 'data-size': 'sm', class: 'folder-add',
      onclick: () => foldersDialog(project, { focusAdd: true }),
    }));
    // 会话：默认按项目过滤出活跃（未归档）+孤立树。
    const filtered = sessions.filter((s) => titleText(s).toLowerCase().includes(filter));
    const active = filtered.filter((s) => !s.archived);
    const archived = filtered.filter((s) => s.archived);
    if (!active.length && !archived.length) {
      children.append(h('p', { class: 'session-empty' },
        filter ? '该项目下没有匹配的会话。' : '该项目还没有会话，「＋ 新任务」开始第一个。'));
      children.append(button('＋ 新任务', {
        'data-variant': 'ghost', 'data-size': 'sm', class: 'project-new-session',
        onclick: () => onNewSession?.(project.id),
      }));
      return children;
    }
    for (const group of groupSessions(active)) {
      children.append(sectionLabel(group.title, group.sessions.length));
      for (const { session, depth } of treeRows(group.sessions)) {
        children.append(sessionRow(session, depth));
      }
    }
    if (archived.length) {
      const head = h('button', {
        type: 'button', class: 'session-group-label is-toggle',
        'aria-expanded': String(archiveOpen),
        onclick: () => { archiveOpen = !archiveOpen; render(); },
      },
        h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, archiveOpen ? '▾' : '▸'),
        h('span', {}, '归档'),
        h('span', { class: 'count' }, String(archived.length)));
      children.append(head);
      if (archiveOpen) {
        for (const session of archived) children.append(sessionRow(session));
      }
    }
    children.append(button('＋ 新任务', {
      'data-variant': 'ghost', 'data-size': 'sm', class: 'project-new-session',
      onclick: () => onNewSession?.(project.id),
    }));
    return children;
  }

  function sectionLabel(title, count) {
    return h('div', { class: 'session-group-label' },
      h('span', {}, title),
      count !== undefined ? h('span', { class: 'count' }, String(count)) : null);
  }

  /* ---------- 会话行（含附属面板折叠行） ---------- */
  function sessionRow(session, depth = 0) {
    const id = session.id;
    const running = state.runningSessions.has(id);
    const selected = id === currentId;
    const attachOpen = attachOpenSet.has(id);
    const row = h('div', {
      class: cn('session-row', depth > 0 && 'is-child'),
      style: depth > 0 ? { '--depth': String(depth) } : null,
    });
    const main = h('button', {
      type: 'button',
      class: 'session-row-main',
      'aria-current': selected ? 'page' : 'false',
      onclick: () => onSelect(id, { projectId: session.project_id }),
    },
      depth > 0 ? h('span', { class: 'tree-guide', 'aria-hidden': 'true' }, '↳') : null,
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
    // 附属面板切换箭头：默认折叠。
    const attachToggle = button(attachOpen ? '▾' : '▸', {
      'data-variant': 'ghost', 'data-size': 'icon-sm',
      class: 'session-attach-toggle', 'aria-label': '附属面板',
      'aria-expanded': String(attachOpen),
      onclick: (event) => {
        event.stopPropagation();
        if (attachOpen) attachOpenSet.delete(id);
        else attachOpenSet.add(id);
        render();
      },
    });
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm',
      class: 'session-menu', 'aria-label': '会话操作',
      onclick: (event) => { event.stopPropagation(); rowMenu(session, event.currentTarget); },
    });
    row.append(main, attachToggle, menuBtn);
    if (attachOpen) row.append(attachRow(session));
    return row;
  }

  // 附属行：终端/浏览器绑定 chips + 打开按钮。默认折叠（attachOpen 控制）。
  function adoptAttachments(session) {
    return attachPanel?.counts?.(session.id) || { terms: 0, tabs: 0 };
  }

  function attachRow(session) {
    const counts = adoptAttachments(session);
    const row = h('div', { class: 'attach-row', role: 'group', 'aria-label': '附属面板' });
    row.append(
      button(`终端 ${counts.terms ? `· ${counts.terms}` : ''} ＋`, {
        'data-variant': 'secondary', 'data-size': 'sm', class: 'attach-btn',
        onclick: () => attachPanel?.openTerminal?.(session.id),
        title: '打开/恢复该会话的附属终端',
      }),
      button(`浏览器 ${counts.tabs ? `· ${counts.tabs}` : ''} ＋`, {
        'data-variant': 'secondary', 'data-size': 'sm', class: 'attach-btn',
        onclick: () => attachPanel?.openBrowser?.(session.id),
        title: '打开/恢复该会话的附属浏览器标签',
      }),
    );
    if (!counts.terms && !counts.tabs) {
      row.append(h('span', { class: 'text-ui-xs text-subtle' }, '尚无附属面板（按会话绑定）'));
    }
    return row;
  }

  function rowMenu(session, anchor) {
    const archived = Boolean(session.archived);
    openMenu(anchor, [
      { label: '打开会话', icon: '↗', onSelect: () => onSelect(session.id, { projectId: session.project_id }) },
      { label: '移动到项目…', onSelect: () => moveToProjectMenu(session, anchor, () => refresh()) },
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
      toast(archived ? '已归档，可在项目展开区中找到' : '已取消归档');
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
              attachOpen.delete(session.id);
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
      onSelect(fork.id, { projectId: fork.project_id });
    } catch (error) { toast(`分支失败：${error.message}`, 'error'); }
  }

  // 子对话树：父会话在列表内时子会话缩进挂载（fork 链），否则平铺。
  function treeRows(sessions) {
    const byId = new Map(sessions.map((s) => [s.id, s]));
    const children = new Map();
    const roots = [];
    for (const s of sessions) {
      if (s.parent_id && byId.has(s.parent_id)) {
        if (!children.has(s.parent_id)) children.set(s.parent_id, []);
        children.get(s.parent_id).push(s);
      } else {
        roots.push(s);
      }
    }
    const rows = [];
    const walk = (list, depth) => {
      for (const s of list) {
        rows.push({ session: s, depth });
        const kids = children.get(s.id);
        if (kids?.length) walk(kids, Math.min(depth + 1, 3));
      }
    };
    walk(roots, 0);
    return rows;
  }

  /* ---------- 项目文件夹 / 主目录 ---------- */
  async function foldersDialog(project, { focusAdd = false } = {}) {
    const listEl = h('div', { class: 'flex flex-col gap-2' });
    const paths = () => [...(project.folders || [])];
    let pendingEntry = project.entry || null;
    function rowEl(folder) {
      const isEntry = pendingEntry === folder;
      const entryBtn = button(isEntry ? '★ 主目录' : '设为主目录', {
        'data-variant': isEntry ? 'outline' : 'ghost', 'data-size': 'sm',
        onclick: async () => {
          pendingEntry = isEntry ? null : folder;
          await persist();
        },
        title: '主目录是新会话终端和文件面板的默认起始目录',
      });
      return h('div', { class: 'folder-manage-row' },
        h('code', { class: 'mono min-w-0 truncate' }, folder),
        entryBtn,
        button('移除', {
          'data-variant': 'ghost', 'data-size': 'sm',
          onclick: async () => {
            const next = paths().filter((item) => item !== folder);
            if (pendingEntry === folder) pendingEntry = null;
            await persist(next, pendingEntry);
          },
        }),
      );
    }

    async function persist(nextFolders, nextEntry) {
      try {
        const body = { };
        if (Array.isArray(nextFolders)) body.folders = nextFolders;
        // entry 语义：undefined=不动；null=清除；字符串=设置。
        if (nextEntry !== undefined) body.entry = nextEntry;
        if (body.folders || body.entry !== undefined && Object.keys(body).length) {
          await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, body);
          project.folders = body.folders ?? project.folders;
          project.entry = body.entry !== undefined ? body.entry : project.entry;
        }
        await refresh();
        repaint();
        notifyChanged?.();
      } catch (error) { toast(`保存失败：${error.message}`, 'error'); }
    }

    function repaint() {
      listEl.replaceChildren();
      const item = paths();
      if (!item.length) listEl.append(h('p', { class: 'text-ui-sm text-muted' }, '还没有文件夹。添加后该项目的终端/文件面板可以直达对应目录。'));
      for (const folder of item) listEl.append(rowEl(folder));
    }

    const input = h('input', {
      'data-component': 'input', value: '', maxlength: '300',
      placeholder: '相对路径，例如 apps/frontend 或 docs',
      'aria-label': '新文件夹路径', spellcheck: 'false',
    });
    async function addFolder() {
      const value = input.value.trim();
      if (!value) { input.setAttribute('aria-invalid', 'true'); return; }
      const next = [...paths(), value];
      await persist(next, pendingEntry);
      input.value = '';
    }
    input.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); addFolder(); }
    });

    const dialog = openDialog({
      title: `项目文件夹 · ${project.name}`,
      description: '项目可关联 workspace 内多个文件夹作为常驻上下文；主目录将是新会话的默认起始目录。',
      size: 'lg',
      body: h('div', { class: 'flex flex-col gap-3' }, listEl,
        h('div', { class: 'flex gap-2' }, input, button('添加文件夹', { 'data-variant': 'default', onclick: addFolder }))),
    });
    repaint();
    if (focusAdd) input.focus();
    return dialog;
  }

  async function entryDialog(project) {
    // 快速切换主目录：复用文件夹对话框即可。
    return foldersDialog(project);
  }

  function renameProjectDialog(project) {
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
              notifyChanged?.();
              toast('已重命名项目');
            } catch (error) { toast(`重命名失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  /* ---------- 顶层渲染 ---------- */
  function render() {
    list.replaceChildren();
    const projects = [...state.projects].sort((a, b) => String(a.created_at).localeCompare(String(b.created_at)));
    // 未分类会话挂在默认项目树之外的一个“未分类”分组；无会话的项目不显示分组行。
    const orphans = state.sessions.filter((s) => s && !s.project_id);
    if (!projects.length && !orphans.length) {
      list.append(h('p', { class: 'session-empty' }, '还没有项目或会话。创建项目开始组织。'));
      return;
    }
    for (const project of projects) list.append(projectRow(project));
    if (orphans.length) {
      const orphanGroup = h('div', { class: 'project-row is-orphans' });
      const anyOrphanMatch = orphans.some((s) => titleText(s).toLowerCase().includes(filter));
      const head = h('button', {
        type: 'button', class: 'project-row-main',
        'aria-expanded': String(projectOpenState({ open: true, id: '__orphans__' })),
      },
        h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, '▾'),
        h('span', { class: 'project-chip-dot', style: { background: 'transparent', border: '1px dashed var(--border-strong)' } }),
        h('span', { class: 'project-name min-w-0' }, '未分类'),
        h('span', { class: 'count mono' }, String(orphans.length)));
      head.addEventListener('click', () => { render(); }); // 占位：children 总是显示（保持轻量）。
      orphanGroup.append(head, projectOrphansChildren(orphans));
      if (anyOrphanMatch) list.append(orphanGroup);
    }
  }

  function projectOrphansChildren(orphans) {
    const children = h('div', { class: 'project-children' });
    for (const { session, depth } of treeRows(orphans)) children.append(sessionRow(session, depth));
    return children;
  }

  async function refresh() {
    await refreshProjects();
    try {
      state.sessions = await api('/api/sessions');
    } catch (_) { /* 连接错误由顶栏状态指示 */ }
    render();
  }

  on('sessions-changed', render);
  on('running-changed', render);

  container.append(
    h('div', { class: 'session-search' }, searchInput, manageBtn),
    list,
  );
  render();

  return {
    refresh,
    setCurrent(id) { currentId = id; render(); },
    get currentId() { return currentId; },
    setAttachPanel,
  };
}

// 未分类会话仍可移动到项目；未分类分组行的 children 总是显示（保持轻量）。
