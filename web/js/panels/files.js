// 文件管理面板：workspace 目录树（惰性加载）+ 文本编辑保存 + 图片预览
// + 新建文件/目录、重命名、删除。所有路径由服务端限制在 workspace 根内。

import { h, cn, button, badge, openMenu, openDialog, toast, armConfirm } from '../ui.js';
import { api, apiPost, apiPath } from '../api.js';

const IMAGE_EXT = /\.(png|jpe?g|gif|svg|webp|ico|bmp)$/i;
const EDITABLE_MAX = 1024 * 1024;

export function createFilesPanel({ container }) {
  let cwd = '';            // 当前树根（相对 workspace）
  let selected = '';       // 选中的文件
  const expanded = new Set();
  const cache = new Map(); // path -> entries[]

  const breadcrumb = h('div', { class: 'fs-breadcrumb mono text-ui-xs', 'aria-label': '当前目录' });
  const tree = h('div', { class: 'fs-tree min-h-0', role: 'tree', 'aria-label': '目录树' });
  const preview = h('div', { class: 'fs-preview min-h-0', 'aria-label': '文件预览' });

  function join(dir, name) { return dir ? `${dir}/${name}` : name; }
  function parentOf(path) {
    const idx = path.lastIndexOf('/');
    return idx === -1 ? '' : path.slice(0, idx);
  }
  function baseName(path) { return path.slice(path.lastIndexOf('/') + 1) || path || 'workspace'; }

  function renderBreadcrumb() {
    breadcrumb.replaceChildren();
    const rootBtn = h('button', { type: 'button', class: 'fs-crumb', onclick: () => setCwd('') }, 'workspace');
    breadcrumb.append(rootBtn);
    let acc = '';
    for (const seg of cwd.split('/').filter(Boolean)) {
      breadcrumb.append(h('span', { class: 'text-subtle' }, ' / '));
      acc = acc ? `${acc}/${seg}` : seg;
      const path = acc;
      breadcrumb.append(h('button', {
        type: 'button', class: 'fs-crumb', onclick: () => setCwd(path),
      }, seg));
    }
  }

  async function listDir(path) {
    if (cache.has(path)) return cache.get(path);
    const data = await api(`/api/fs?path=${encodeURIComponent(path)}`);
    const entries = Array.isArray(data.entries) ? data.entries : [];
    cache.set(path, entries);
    return entries;
  }

  function invalidate(path) {
    cache.delete(path);
    cache.delete(parentOf(path));
  }

  function row(entry, path, depth) {
    const isDir = entry.kind === 'dir';
    const isOpen = expanded.has(path);
    const rowEl = h('div', {
      class: cn('fs-row', selected === path && 'is-selected'),
      role: 'treeitem',
      'aria-expanded': isDir ? String(isOpen) : undefined,
      'aria-selected': selected === path ? 'true' : 'false',
      style: { '--depth': String(depth) },
      tabindex: '0',
      onclick: () => (isDir ? toggleDir(path) : openFile(path)),
      onkeydown: (e) => {
        if (e.key === 'Enter') isDir ? toggleDir(path) : openFile(path);
      },
    },
      h('span', { class: 'fs-chevron text-subtle', 'aria-hidden': 'true' }, isDir ? (isOpen ? '▾' : '▸') : ''),
      h('span', { class: 'fs-icon', 'aria-hidden': 'true' }, isDir ? '🗀' : IMAGE_EXT.test(path) ? '🖼' : '🗎'),
      h('span', { class: 'fs-name min-w-0' }, entry.name),
      entry.kind === 'link' ? badge('链接', 'outline') : null,
    );
    const menuBtn = button('⋯', {
      'data-variant': 'ghost', 'data-size': 'icon-sm', class: 'fs-menu', 'aria-label': '文件操作',
      onclick: (event) => { event.stopPropagation(); entryMenu(entry, path, event.currentTarget); },
    });
    rowEl.append(menuBtn);
    const frag = [rowEl];
    if (isDir && isOpen) {
      const kids = cache.get(path) || [];
      for (const kid of kids) frag.push(...row(kid, join(path, kid.name), depth + 1));
    }
    return frag;
  }

  function entryMenu(entry, path, anchor) {
    openMenu(anchor, [
      entry.kind === 'dir'
        ? { label: '刷新目录', onSelect: () => { cache.delete(path); expanded.add(path); render(); } }
        : { label: '打开文件', onSelect: () => openFile(path) },
      { label: '重命名…', onSelect: () => renameDialog(entry, path) },
      '-',
      { label: '删除…', variant: 'destructive', onSelect: () => deleteDialog(entry, path) },
    ]);
  }

  async function toggleDir(path) {
    if (expanded.has(path)) expanded.delete(path);
    else {
      try { await listDir(path); expanded.add(path); }
      catch (error) { toast(`读取目录失败：${error.message}`, 'error'); return; }
    }
    render();
  }

  async function openFile(path) {
    selected = path;
    render();
    if (IMAGE_EXT.test(path)) {
      preview.replaceChildren(
        h('div', { class: 'fs-preview-head' }, h('span', { class: 'mono text-ui-xs' }, path)),
        h('img', { class: 'fs-image', src: apiPath(`/api/file/raw?path=${encodeURIComponent(path)}`), alt: path }),
      );
      return;
    }
    try {
      const data = await api(`/api/file?path=${encodeURIComponent(path)}`);
      if (data.binary) {
        preview.replaceChildren(
          h('div', { class: 'fs-preview-head' }, h('span', { class: 'mono text-ui-xs' }, path), badge('二进制', 'outline')),
          h('p', { class: 'text-ui-sm text-subtle' }, '二进制文件不支持内联预览。'),
          h('a', { class: 'fs-download', href: apiPath(`/api/file/raw?path=${encodeURIComponent(path)}`), download: baseName(path) }, '下载原始文件'),
        );
        return;
      }
      const editor = h('textarea', {
        class: 'fs-editor mono', spellcheck: 'false',
        'aria-label': '文件内容编辑',
      });
      editor.value = data.content ?? '';
      const dirty = () => saveBtn.disabled = editor.value === (data.content ?? '');
      const saveBtn = button('保存', {
        'data-variant': 'default', 'data-size': 'sm', disabled: true,
        onclick: async () => {
          try {
            await api('/api/file', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, content: editor.value }) });
            data.content = editor.value;
            dirty();
            toast(`已保存 ${baseName(path)}`);
          } catch (error) { toast(`保存失败：${error.message}`, 'error'); }
        },
      });
      editor.addEventListener('input', dirty);
      preview.replaceChildren(
        h('div', { class: 'fs-preview-head' },
          h('span', { class: 'mono text-ui-xs min-w-0' }, path),
          data.truncated ? badge('已截断', 'outline') : null,
          h('span', { class: 'push-end' }),
          saveBtn,
        ),
        editor,
      );
    } catch (error) { toast(`读取失败：${error.message}`, 'error'); }
  }

  function newFileDialog(kind) {
    const input = h('input', { 'data-component': 'input', placeholder: kind === 'dir' ? '目录路径，如 src/lib' : '文件路径，如 notes/todo.md', 'aria-label': '新路径' });
    const dialog = openDialog({
      title: kind === 'dir' ? '新建目录' : '新建文件',
      description: `相对 workspace 根；父目录不存在时自动创建。`,
      body: h('div', { class: 'flex flex-col gap-2' }, input),
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('创建', {
          'data-variant': 'default',
          onclick: async () => {
            const path = input.value.trim().replace(/^\/+/, '');
            if (!path) { input.setAttribute('aria-invalid', 'true'); return; }
            try {
              if (kind === 'dir') await apiPost('/api/fs/mkdir', { path });
              else await api('/api/file', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, content: '' }) });
              dialog.close();
              cache.clear();
              expanded.add(parentOf(path));
              render();
              toast(`已创建 ${path}`);
            } catch (error) { toast(`创建失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  function renameDialog(entry, path) {
    const input = h('input', { 'data-component': 'input', value: path, 'aria-label': '新路径' });
    const dialog = openDialog({
      title: '重命名 / 移动',
      description: '可改为新名称或移动到其他目录（workspace 内）。',
      body: h('div', { class: 'flex flex-col gap-2' }, input),
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('应用', {
          'data-variant': 'default',
          onclick: async () => {
            const to = input.value.trim().replace(/^\/+/, '');
            if (!to || to === path) return;
            try {
              await apiPost('/api/fs/rename', { from: path, to });
              dialog.close();
              cache.clear();
              if (selected === path) selected = to;
              render();
              toast(`已重命名为 ${to}`);
            } catch (error) { toast(`重命名失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  function deleteDialog(entry, path) {
    const isDir = entry.kind === 'dir';
    const dialog = openDialog({
      title: isDir ? '删除目录' : '删除文件',
      description: `“${path}”${isDir ? ' 及其全部内容' : ''}将被永久删除，此操作不可撤销。`,
      footer: [
        button('取消', { 'data-variant': 'ghost', onclick: () => dialog.close() }),
        button('永久删除', {
          'data-variant': 'destructive',
          onclick: async () => {
            try {
              await apiDelete(`/api/fs?path=${encodeURIComponent(path)}`);
              dialog.close();
              cache.clear();
              expanded.delete(path);
              if (selected === path) { selected = ''; preview.replaceChildren(); }
              render();
              toast('已删除');
            } catch (error) { toast(`删除失败：${error.message}`, 'error'); }
          },
        }),
      ],
    });
  }

  function render() {
    renderBreadcrumb();
    tree.replaceChildren();
    const entries = cache.get(cwd) || [];
    if (!cache.has(cwd)) {
      tree.append(h('p', { class: 'session-empty' }, '正在读取…'));
    } else if (!entries.length) {
      tree.append(h('p', { class: 'session-empty' }, '目录为空。'));
    } else {
      for (const entry of entries) tree.append(...row(entry, join(cwd, entry.name), 0));
    }
  }

  async function setCwd(path) {
    cwd = path;
    try { await listDir(path); } catch (error) { toast(`读取目录失败：${error.message}`, 'error'); }
    render();
  }

  const refreshBtn = button('↻', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '刷新', title: '刷新目录树',
    onclick: async () => { cache.clear(); await listDir(cwd).catch(() => {}); render(); },
  });
  const newFileBtn = button('新文件', { 'data-variant': 'ghost', 'data-size': 'sm', onclick: () => newFileDialog('file') });
  const newDirBtn = button('新目录', { 'data-variant': 'ghost', 'data-size': 'sm', onclick: () => newFileDialog('dir') });

  container.replaceChildren(
    h('div', { class: 'sidebar-toolbar' }, breadcrumb, h('span', { class: 'push-end' }), refreshBtn, newFileBtn, newDirBtn),
    tree,
    preview,
  );

  return {
    async refresh() {
      // 刷新 = 失效缓存后重列根目录、当前目录与全部展开目录（切工作区后路径空间已变）。
      cache.clear();
      const targets = [...new Set(['', cwd, ...expanded])];
      await Promise.all(targets.map((p) => listDir(p).catch(() => {})));
      render();
    },
    show() { return this.refresh(); },
    hide() {},
  };
}
