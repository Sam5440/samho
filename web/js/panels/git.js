// Git 审查面板：变更清单（暂存/未暂存/未跟踪）+ 统一 diff + 提交 + 历史
// + 分支切换。所有操作走 /api/git/*，丢弃修改需两击确认。

import { h, cn, button, badge, openMenu, openDialog, toast, armConfirm, relativeTime } from '../ui.js';
import { api, apiPost } from '../api.js';

const STATUS_LABEL = {
  M: '修改', A: '新增', D: '删除', R: '重命名', C: '复制', U: '冲突', '?': '未跟踪', '!': '丢失',
};

export function createGitPanel({ container }) {
  let branch = '';
  let upstream = null;
  let changes = [];
  let commits = [];
  let branches = [];
  let selected = null;   // { path, staged }
  let diffText = '';
  let showLog = false;

  const branchBtn = button('…', {
    'data-variant': 'ghost', 'data-size': 'sm', class: 'git-branch', 'aria-label': '切换分支',
    onclick: (event) => branchesMenu(event.currentTarget),
  });
  const refreshBtn = button('↻', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '刷新', title: '刷新状态与历史',
    onclick: () => refresh(),
  });
  const logBtn = button('历史', {
    'data-variant': 'ghost', 'data-size': 'sm', 'aria-expanded': 'false',
    onclick: () => { showLog = !showLog; logBtn.setAttribute('aria-expanded', String(showLog)); render(); },
  });
  const stageAllBtn = button('暂存全部', {
    'data-variant': 'outline', 'data-size': 'sm',
    onclick: async () => {
      const paths = changes.filter((c) => !c.staged).map((c) => c.path);
      if (!paths.length) { toast('没有可暂存的变更'); return; }
      try { await apiPost('/api/git/stage', { paths }); await refresh(); toast(`已暂存 ${paths.length} 项`); }
      catch (error) { toast(`暂存失败：${error.message}`, 'error'); }
    },
  });

  const statusList = h('div', { class: 'git-list min-h-0', role: 'list', 'aria-label': '变更清单' });
  const diffView = h('div', { class: 'diff-view mono min-h-0', 'aria-label': '差异' });
  const logList = h('div', { class: 'git-log min-h-0', hidden: true, 'aria-label': '提交历史' });
  const commitInput = h('textarea', { class: 'git-commit-input', rows: '2', placeholder: '提交说明…（提交已暂存变更，或先在文件菜单选择路径）', 'aria-label': '提交说明' });
  const commitBtn = button('提交', {
    'data-variant': 'default', 'data-size': 'sm',
    onclick: commitNow,
  });
  commitInput.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') commitNow();
  });

  async function commitNow() {
    const message = commitInput.value.trim();
    if (!message) { commitInput.setAttribute('aria-invalid', 'true'); commitInput.focus(); return; }
    try {
      const result = await apiPost('/api/git/commit', { message, paths: [] });
      commitInput.value = '';
      await refresh();
      toast(`已提交 ${result.hash || ''}`);
    } catch (error) { toast(`提交失败：${error.message}`, 'error'); }
  }

  function branchesMenu(anchor) {
    const items = branches.map((b) => ({
      label: b.current ? `● ${b.name}` : b.name,
      onSelect: async () => {
        try { await apiPost('/api/git/checkout', { branch: b.name }); await refresh(); toast(`已切换到 ${b.name}`); }
        catch (error) { toast(`切换失败：${error.message}`, 'error'); }
      },
    }));
    items.push('-', {
      label: '新建分支…',
      onSelect: () => newBranchDialog(),
    });
    openMenu(anchor, items);
  }

  function newBranchDialog() {
    const input = h('input', { 'data-component': 'input', placeholder: '新分支名', 'aria-label': '新分支名' });
    const dialog = openDialog({
      title: '从当前提交新建分支',
      body: h('div', { class: 'flex flex-col gap-2' }, input),
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('创建并切换', {
          'data-variant': 'default',
          onclick: async () => {
            const name = input.value.trim();
            if (!name) { input.setAttribute('aria-invalid', 'true'); return; }
            try {
              await apiPost('/api/git/checkout', { branch: name, create: true });
              dialog.close();
              await refresh();
              toast(`已创建并切换到 ${name}`);
            } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  function changeRow(change) {
    const isActive = selected && selected.path === change.path && selected.staged === change.staged;
    const x = String(change.x);
    const y = String(change.y);
    const code = change.untracked ? '??' : change.staged ? x : y;
    const row = h('div', {
      class: cn('git-row', isActive && 'is-selected'),
      role: 'listitem',
      tabindex: '0',
      onclick: () => selectChange(change),
      onkeydown: (e) => { if (e.key === 'Enter') selectChange(change); },
    },
      h('span', { class: cn('git-x mono', change.untracked && 'text-subtle') }, code),
      h('span', { class: 'git-path mono min-w-0' }, change.path),
      badge(STATUS_LABEL[change.staged ? x : y] || '变更', change.staged ? 'solid' : 'outline'),
    );
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm', class: 'git-menu', 'aria-label': '变更操作',
      onclick: (event) => { event.stopPropagation(); changeMenu(change, event.currentTarget); },
    });
    row.append(menuBtn);
    return row;
  }

  function changeMenu(change, anchor) {
    openMenu(anchor, [
      { label: '查看差异', onSelect: () => selectChange(change) },
      change.staged
        ? { label: '取消暂存', onSelect: () => unstage(change) }
        : { label: '暂存', onSelect: () => stage([change.path]) },
      change.staged ? { label: '暂存（含工作区）', onSelect: () => stage([change.path]) } : null,
      '-',
      { label: '丢弃修改…', variant: 'destructive', onSelect: () => discardDialog(change) },
    ].filter(Boolean));
  }

  async function stage(paths) {
    try { await apiPost('/api/git/stage', { paths }); await refresh(); }
    catch (error) { toast(`暂存失败：${error.message}`, 'error'); }
  }
  async function unstage(change) {
    try { await apiPost('/api/git/unstage', { paths: [change.path] }); await refresh(); }
    catch (error) { toast(`取消暂存失败：${error.message}`, 'error'); }
  }

  function discardDialog(change) {
    const dialog = openDialog({
      title: '丢弃修改',
      description: `“${change.path}” 的未暂存修改将被永久丢弃。`,
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('丢弃', {
          'data-variant': 'destructive',
          onclick: async () => {
            try {
              await apiPost('/api/git/discard', { paths: [change.path] });
              dialog.close();
              await refresh();
              toast('已丢弃修改');
            } catch (error) { toast(`丢弃失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  async function selectChange(change) {
    selected = { path: change.path, staged: change.staged };
    diffView.replaceChildren(h('p', { class: 'session-empty' }, '正在读取差异…'));
    renderStatus();
    try {
      const data = await api(`/api/git/diff?path=${encodeURIComponent(change.path)}&staged=${change.staged}`);
      diffText = data.diff || '';
      renderDiff();
    } catch (error) {
      diffView.replaceChildren(h('p', { class: 'session-empty' }, `差异读取失败：${error.message}`));
    }
  }

  function renderDiff() {
    if (!selected) {
      diffView.replaceChildren(h('p', { class: 'session-empty' }, '选择一个变更查看差异。'));
      return;
    }
    const head = h('div', { class: 'diff-head' },
      h('span', { class: 'mono text-ui-xs min-w-0' }, `${selected.staged ? '已暂存' : '工作区'} · ${selected.path}`),
      h('button', {
        type: 'button', 'data-component': 'button', 'data-variant': 'ghost', 'data-size': 'icon-sm',
        'aria-label': '关闭差异', onclick: () => { selected = null; diffText = ''; renderStatus(); renderDiff(); },
      }, '✕'),
    );
    if (!diffText.trim()) {
      diffView.replaceChildren(head, h('p', { class: 'session-empty' }, '没有差异内容（未跟踪文件请先暂存）。'));
      return;
    }
    const body = h('div', { class: 'diff-body' });
    for (const line of diffText.split('\n')) {
      let cls = 'diff-line';
      let marker = ' ';
      if (/^(diff |index |--- |\+\+\+ )/.test(line)) { cls += ' is-meta'; }
      else if (line.startsWith('+')) { cls += ' is-added'; marker = '+'; }
      else if (line.startsWith('-')) { cls += ' is-removed'; marker = '-'; }
      else if (line.startsWith('@@')) { cls += ' is-hunk'; }
      body.append(h('div', { class: cls }, h('span', { class: 'diff-marker', 'aria-hidden': 'true' }, cls.includes('is-hunk') ? '' : marker), h('span', { class: 'diff-text' }, line || ' ')));
    }
    diffView.replaceChildren(head, body);
  }

  function renderStatus() {
    statusList.replaceChildren();
    if (!changes.length) {
      statusList.append(h('p', { class: 'session-empty' }, '工作区干净，没有未提交变更。'));
    } else {
      for (const change of changes) statusList.append(changeRow(change));
    }
  }

  function renderLog() {
    logList.replaceChildren();
    if (!commits.length) {
      logList.append(h('p', { class: 'session-empty' }, '还没有提交历史。'));
      return;
    }
    for (const c of commits) {
      logList.append(h('div', { class: 'git-commit-row' },
        h('span', { class: 'git-hash mono' }, c.short),
        h('span', { class: 'git-subject min-w-0' }, c.subject || '（无说明）'),
        c.refs ? badge(c.refs, 'outline') : null,
        h('span', { class: 'text-subtle text-ui-xs push-end' }, `${c.author} · ${relativeTime(c.time * 1000)}`),
      ));
    }
  }

  function render() {
    branchBtn.textContent = branch || '（无分支）';
    renderStatus();
    renderDiff();
    renderLog();
    logList.hidden = !showLog;
  }

  async function refresh() {
    try {
      const [status, log, branchData] = await Promise.all([
        api('/api/git/status'),
        api('/api/git/log?limit=20').catch(() => ({ commits: [] })),
        api('/api/git/branches').catch(() => ({ branches: [] })),
      ]);
      branch = status.branch || '';
      upstream = status.upstream ?? null;
      changes = Array.isArray(status.changes) ? status.changes : [];
      commits = log.commits || [];
      branches = branchData.branches || [];
      if (selected && !changes.some((c) => c.path === selected.path && c.staged === selected.staged)) {
        selected = null;
        diffText = '';
      }
      render();
    } catch (error) {
      statusList.replaceChildren(h('p', { class: 'session-empty' }, `Git 不可用：${error.message}`));
    }
  }

  container.replaceChildren(
    h('div', { class: 'sidebar-toolbar' }, branchBtn, h('span', { class: 'push-end' }), logBtn, stageAllBtn, refreshBtn),
    statusList,
    diffView,
    logList,
    h('div', { class: 'git-commit' }, commitInput, commitBtn),
  );

  return {
    refresh,
    show() { return refresh(); },
    hide() {},
  };
}
