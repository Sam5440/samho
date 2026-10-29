// 终端面板：每个工作区一组 PTY 会话（服务端 portable-pty）。
// xterm.js 从 /vendor 惰性加载（CSP 'self'）；切工作区保留各自滚动缓冲，
// 服务端会话闲置 30 分钟自动回收，输入 404 时自动重建。

import { h, cn, button, toast } from '../ui.js';
import { api, apiPost, apiPath, getWorkspacePrefix } from '../api.js';

let xtermPromise = null;
function loadScript(src) {
  return new Promise((resolve, reject) => {
    const script = document.createElement('script');
    script.src = src;
    script.onload = resolve;
    script.onerror = () => reject(new Error(`脚本加载失败：${src}`));
    document.head.append(script);
  });
}
function ensureXterm() {
  if (xtermPromise) return xtermPromise;
  xtermPromise = (async () => {
    if (!window.Terminal) {
      const css = document.createElement('link');
      css.rel = 'stylesheet';
      css.href = '/vendor/xterm.css';
      document.head.append(css);
      await loadScript('/vendor/xterm.js');
    }
    if (!window.Terminal) throw new Error('xterm 加载失败');
    // 新版 Chromium 下 DOM 渲染器出空行，canvas 渲染器兜底（失败则仍回 DOM）。
    if (!window.CanvasAddon) {
      try { await loadScript('/vendor/xterm-addon-canvas.js'); } catch (_) { /* 降级 DOM 渲染 */ }
    }
    return window.Terminal;
  })();
  return xtermPromise;
}

// 依据容器尺寸估算 cols/rows（避免引入 addon-fit）。
function fitTerm(term, host) {
  const probe = h('span', {
    class: 'mono', style: { position: 'absolute', visibility: 'hidden', whiteSpace: 'pre', fontSize: '13px', lineHeight: '1.25' },
  }, 'W'.repeat(20));
  host.append(probe);
  const cellW = probe.getBoundingClientRect().width / 20 || 7.8;
  const cellH = Number(term.option('lineHeight') || 1) * 13 * 1.25;
  probe.remove();
  const cols = Math.max(20, Math.min(200, Math.floor(host.clientWidth / cellW) - 1));
  const rows = Math.max(6, Math.min(80, Math.floor(host.clientHeight / cellH) - 1));
  return { cols, rows };
}

export function createTerminalPane({ container }) {
  // prefix -> { id, term, host, es, clean }
  const sessions = new Map();
  let activePrefix = null;
  let visible = false;

  const strip = h('div', { class: 'term-strip', role: 'tablist', 'aria-label': '终端会话' });
  const body = h('div', { class: 'term-body min-h-0' });
  const newBtn = button('＋ 新终端', {
    'data-variant': 'ghost', 'data-size': 'sm',
    onclick: () => openSession(true),
  });
  const killBtn = button('✕ 结束', {
    'data-variant': 'ghost', 'data-size': 'sm', 'aria-label': '结束当前终端',
    onclick: () => killActive(),
  });

  function current() { return sessions.get(activePrefix) || null; }

  function renderStrip() {
    strip.replaceChildren();
    for (const [prefix, session] of sessions) {
      const label = prefix ? prefix.replace('/w/', '') : '默认';
      strip.append(h('div', { class: cn('term-chip', prefix === activePrefix && 'is-active'), role: 'tab', 'aria-selected': String(prefix === activePrefix) },
        h('button', {
          type: 'button', class: 'term-chip-main mono', title: session.title || label,
          onclick: () => focus(prefix),
        }, `${label} · ${(session.id || '?').slice(0, 6)}`),
        h('button', {
          type: 'button', class: 'term-chip-close', 'aria-label': '关闭终端',
          onclick: () => closeSession(prefix),
        }, '✕'),
      ));
    }
  }

  function attach(session) {
    body.replaceChildren(session.host);
    session.host.classList.remove('hidden');
    requestAnimationFrame(() => {
      const { cols, rows } = fitTerm(session.term, session.host);
      if (session.cols !== cols || session.rows !== rows) resize(session, cols, rows);
      session.term.focus();
    });
  }

  function focus(prefix) {
    const session = sessions.get(prefix);
    if (!session) return;
    activePrefix = prefix;
    if (!session.es && session.id) subscribe(session);
    attach(session);
    renderStrip();
  }

  function subscribe(session) {
    const es = new EventSource(apiPath(`/api/term/${encodeURIComponent(session.id)}/stream`));
    es.addEventListener('backfill', (event) => {
      session.term.write(String(event.data || ''));
      session.clean = false;
    });
    es.addEventListener('data', (event) => {
      session.term.write(String(event.data || ''));
      session.clean = false;
    });
    es.onerror = () => { /* EventSource 自动重连；会话被服务端回收时输入接口会 404 */ };
    session.es = es;
  }

  async function resize(session, cols, rows) {
    session.cols = cols; session.rows = rows;
    try { session.term.resize(cols, rows); } catch (_) { /* 缓冲已小于目标时忽略 */ }
    if (session.id) {
      try { await apiPost(`/api/term/${encodeURIComponent(session.id)}/resize`, { cols, rows }); }
      catch (_) { /* 会话可能已退出 */ }
    }
  }

  async function openSession(force = false) {
    if (!force && current()) { focus(activePrefix); return; }
    const Terminal = await ensureXterm();
    const host = h('div', { class: 'term-host' });
    const term = new Terminal({
      fontFamily: 'var(--font-mono), Consolas, monospace',
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 4000,
      theme: {
        background: '#00000000',
        foreground: '#e5e5e5',
        cursor: '#e5e5e5',
        selectionBackground: '#404040',
      },
      allowProposedApi: true,
    });
    const session = { id: '', term, host, es: null, clean: true, cols: 0, rows: 0, title: '' };
    // UMD 全局是 { CanvasAddon: class } 包装；两种形状都兼容。
    const CanvasCtor = window.CanvasAddon && window.CanvasAddon.CanvasAddon
      ? window.CanvasAddon.CanvasAddon : window.CanvasAddon;
    if (typeof CanvasCtor === 'function') {
      try { term.loadAddon(new CanvasCtor()); } catch (_) { /* 回落 DOM 渲染器 */ }
    }
    term.onData((text) => sendInput(session, text));
    sessions.set(getWorkspacePrefix() || 'default', session);
    activePrefix = getWorkspacePrefix() || 'default';
    body.replaceChildren(host);
    term.open(host);
    try {
      const created = await apiPost('/api/term', {});
      session.id = String(created.id || '');
      session.title = String(created.shell || 'shell');
    } catch (error) {
      term.writeln(`\x1b[31m终端创建失败：${error.message}\x1b[0m`);
      toast(`终端创建失败：${error.message}`, 'error');
    }
    if (session.id) subscribe(session);
    attach(session);
    renderStrip();
  }

  async function sendInput(session, text) {
    if (!session.id) return;
    try {
      await apiPost(`/api/term/${encodeURIComponent(session.id)}/input`, { text });
    } catch (error) {
      if (error.status === 404) {
        session.id = '';
        try {
          const created = await apiPost('/api/term', {});
          session.id = String(created.id || '');
          if (session.id) { session.term.writeln('\r\n\x1b[2m〔会话已被回收，已重新打开〕\x1b[0m\r\n'); subscribe(session); }
        } catch (_) { session.term.writeln(`\r\n\x1b[31m会话已退出：${error.message}\x1b[0m`); }
      }
    }
  }

  async function closeSession(prefix) {
    const session = sessions.get(prefix);
    if (!session) return;
    sessions.delete(prefix);
    session.es?.close();
    if (session.id) api(`/api/term/${encodeURIComponent(session.id)}`, { method: 'DELETE' }).catch(() => {});
    try { session.term.dispose(); } catch (_) { /* 已释放 */ }
    if (activePrefix === prefix) {
      const next = sessions.keys().next();
      if (!next.done) focus(next.value);
      else { activePrefix = null; body.replaceChildren(h('p', { class: 'session-empty' }, '没有终端。点击「＋ 新终端」启动设备 shell。')); }
    }
    renderStrip();
  }

  async function killActive() {
    if (activePrefix) await closeSession(activePrefix);
  }

  window.addEventListener('resize', () => {
    const session = current();
    if (!session || !visible) return;
    const { cols, rows } = fitTerm(session.term, session.host);
    if (session.cols !== cols || session.rows !== rows) resize(session, cols, rows);
  });

  container.replaceChildren(
    h('div', { class: 'sidebar-toolbar' },
      h('span', { class: 'text-ui-xs text-subtle' }, '设备 PTY'),
      h('span', { class: 'push-end' }),
      newBtn, killBtn),
    strip,
    body,
  );

  return {
    // 首次显示或切回时调用；每个工作区独立会话组。
    async show() {
      visible = true;
      if (!sessions.has(getWorkspacePrefix() || 'default')) {
        await openSession(true);
        return;
      }
      focus(getWorkspacePrefix() || 'default');
    },
    // 切换工作区：仅隐藏（保留缓冲与服务端会话），下次 open 恢复。
    hide() {
      visible = false;
    },
    hasSession() { return sessions.has(getWorkspacePrefix() || 'default'); },
  };
}
