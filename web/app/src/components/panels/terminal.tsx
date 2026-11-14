// 终端面板：每个工作区一组 PTY（服务端 portable-pty），会话附属终端与工作区级终端共存。
// xterm.js 从 /vendor 惰性加载（CSP script-src 'self'）；切工作区保留各自滚动缓冲，
// 服务端会话闲置 30 分钟自动回收，输入 404 时自动重建。

import { useCallback, useEffect, useRef, useState } from 'react';
import { PlusIcon, SquareTerminalIcon, XIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { toast } from '@/components/ui/toaster';
import { EmptyHint, PanelToolbar } from '@/components/panels/shared';
import { api, apiDelete, apiPath, apiPost, getWorkspacePrefix, ApiError } from '@/lib/api';
import { bridge } from '@/lib/bridge';
import { setState, useStore } from '@/lib/store';
import { cn } from '@/lib/utils';

/* eslint-disable @typescript-eslint/no-explicit-any */
declare global {
  interface Window { Terminal?: any; CanvasAddon?: any }
}

let xtermPromise: Promise<any> | null = null;
function loadScript(src: string) {
  return new Promise<void>((resolve, reject) => {
    const script = document.createElement('script');
    script.src = src;
    script.onload = () => resolve();
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
      try { await loadScript('/vendor/xterm-addon-canvas.js'); } catch { /* 降级 DOM 渲染 */ }
    }
    return window.Terminal;
  })();
  xtermPromise.catch(() => { xtermPromise = null; });
  return xtermPromise;
}

const FONT_SIZE = 12.5;
const LINE_HEIGHT = 1.25;

function xtermTheme() {
  const dark = document.documentElement.classList.contains('dark');
  return dark
    ? { background: '#00000000', foreground: '#e4e4e7', cursor: '#e4e4e7', selectionBackground: '#3f3f46' }
    : { background: '#00000000', foreground: '#27272a', cursor: '#27272a', selectionBackground: '#d4d4d8' };
}

// 依据容器尺寸估算 cols/rows（避免引入 addon-fit）。
function fitSize(host: HTMLElement) {
  const probe = document.createElement('span');
  probe.className = 'font-mono';
  Object.assign(probe.style, { position: 'absolute', visibility: 'hidden', whiteSpace: 'pre', fontSize: `${FONT_SIZE}px` });
  probe.textContent = 'W'.repeat(20);
  host.append(probe);
  const cellW = probe.getBoundingClientRect().width / 20 || 7.5;
  probe.remove();
  const cellH = FONT_SIZE * LINE_HEIGHT * 1.2;
  return {
    // 预留 .xterm 内边距（index.css）与滚动条宽度。
    cols: Math.max(20, Math.min(240, Math.floor((host.clientWidth - 8 - 14) / cellW))),
    rows: Math.max(6, Math.min(100, Math.floor((host.clientHeight - 6) / cellH))),
  };
}

type TermSession = {
  key: string;
  workspace: string;
  sessionId: string | null;
  id: string;
  term: any;
  host: HTMLDivElement;
  es: EventSource | null;
  cols: number;
  rows: number;
};

// key = `${workspacePrefix || 'default'}::${sessionId || 'workspace'}`。
const keyOf = (sessionId: string | null) => `${getWorkspacePrefix() || 'default'}::${sessionId || 'workspace'}`;

export function TerminalPanel({ active }: { active: boolean }) {
  const sessions = useRef(new Map<string, TermSession>());
  const bodyRef = useRef<HTMLDivElement>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [, setVersion] = useState(0);
  const theme = useStore((s) => s.ui.theme);
  const activeRef = useRef(active);
  activeRef.current = active;
  const bump = () => { setVersion((v) => v + 1); setState((s) => ({ attachVersion: s.attachVersion + 1 })); };

  const resize = useCallback(async (session: TermSession) => {
    const { cols, rows } = fitSize(session.host);
    if (session.cols === cols && session.rows === rows) return;
    session.cols = cols; session.rows = rows;
    try { session.term.resize(cols, rows); } catch { /* 缓冲已小于目标时忽略 */ }
    if (session.id) apiPost(`/api/term/${encodeURIComponent(session.id)}/resize`, { cols, rows }).catch(() => {});
  }, []);

  const subscribe = (session: TermSession) => {
    session.es?.close();
    const es = new EventSource(apiPath(`/api/term/${encodeURIComponent(session.id)}/stream`));
    es.addEventListener('backfill', (event) => session.term.write(String((event as MessageEvent).data || '')));
    es.addEventListener('data', (event) => session.term.write(String((event as MessageEvent).data || '')));
    // EventSource 自动重连；会话被服务端回收时输入接口会 404。
    session.es = es;
  };

  const attach = useCallback((key: string) => {
    const session = sessions.current.get(key);
    const body = bodyRef.current;
    if (!session || !body) return;
    body.replaceChildren(session.host);
    setActiveKey(key);
    requestAnimationFrame(() => {
      resize(session);
      session.term.focus();
    });
  }, [resize]);

  const sendInput = async (session: TermSession, text: string) => {
    if (!session.id) return;
    try {
      await apiPost(`/api/term/${encodeURIComponent(session.id)}/input`, { text });
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) {
        session.id = '';
        try {
          const created = await apiPost('/api/term', session.sessionId ? { session_id: session.sessionId } : {});
          session.id = String(created.id || '');
          if (session.id) { session.term.writeln('\r\n\x1b[2m〔会话已被回收，已重新打开〕\x1b[0m\r\n'); subscribe(session); }
        } catch (err) { session.term.writeln(`\r\n\x1b[31m会话已退出：${(err as Error).message}\x1b[0m`); }
      }
    }
  };

  // existingId：服务端已有活体 PTY（附属会话重开），直接重连并由 backfill 恢复历史。
  const create = useCallback(async (sessionId: string | null, existingId?: string) => {
    let Terminal;
    try { Terminal = await ensureXterm(); }
    catch (error) { toast(`终端组件加载失败：${(error as Error).message}`, 'error'); return; }
    const baseKey = keyOf(sessionId);
    // 同一 key 已存在时（显式新建），追加序号以并存多个 PTY。
    let key = baseKey;
    for (let n = 2; sessions.current.has(key); n++) key = `${baseKey}#${n}`;
    const host = document.createElement('div');
    host.className = 'term-host absolute inset-0';
    const term = new Terminal({
      fontFamily: '"JetBrains Mono", "Cascadia Code", Consolas, monospace',
      fontSize: FONT_SIZE,
      lineHeight: LINE_HEIGHT,
      cursorBlink: true,
      scrollback: 4000,
      theme: xtermTheme(),
      // 透明背景在 canvas 渲染器下需要显式允许，终端随主题露出面板底色。
      allowTransparency: true,
      allowProposedApi: true,
    });
    const session: TermSession = {
      key, workspace: getWorkspacePrefix() || 'default', sessionId, id: existingId || '', term, host, es: null, cols: 0, rows: 0,
    };
    // UMD 全局是 { CanvasAddon: class } 包装；两种形状都兼容。
    const CanvasCtor = window.CanvasAddon?.CanvasAddon || window.CanvasAddon;
    if (typeof CanvasCtor === 'function') {
      try { term.loadAddon(new CanvasCtor()); } catch { /* 回落 DOM 渲染器 */ }
    }
    term.onData((text: string) => sendInput(session, text));
    sessions.current.set(key, session);
    bodyRef.current?.replaceChildren(host);
    term.open(host);
    if (!session.id) {
      try {
        // 附属终端带 session_id 登记到后端；工作区级终端不传。
        const created = await apiPost('/api/term', sessionId ? { session_id: sessionId } : {});
        session.id = String(created.id || '');
      } catch (error) {
        term.writeln(`\x1b[31m终端创建失败：${(error as Error).message}\x1b[0m`);
        toast(`终端创建失败：${(error as Error).message}`, 'error');
      }
    }
    if (session.id) subscribe(session);
    attach(key);
    bump();
  }, [attach]);

  const close = useCallback((key: string) => {
    const session = sessions.current.get(key);
    if (!session) return;
    sessions.current.delete(key);
    session.es?.close();
    if (session.id) apiDelete(`/api/term/${encodeURIComponent(session.id)}`).catch(() => {});
    try { session.term.dispose(); } catch { /* 已释放 */ }
    const next = [...sessions.current.values()].find((s) => s.workspace === (getWorkspacePrefix() || 'default'));
    if (next) attach(next.key);
    else { bodyRef.current?.replaceChildren(); setActiveKey(null); }
    bump();
  }, [attach]);

  // 显示时：sessionId 提供则聚焦/恢复该会话的附属终端；否则沿用工作区级终端组。
  const show = useCallback(async ({ sessionId = null }: { sessionId?: string | null } = {}) => {
    if (sessionId) {
      const key = keyOf(sessionId);
      if (sessions.current.has(key)) { attach(key); return; }
      try {
        const listing = await api(`/api/term?session_id=${encodeURIComponent(sessionId)}`);
        const terms = Array.isArray(listing?.terms) ? listing.terms : [];
        if (terms.length) { await create(sessionId, String(terms[0].id)); return; }
      } catch { /* 探活失败 → 走新建流程 */ }
      await create(sessionId);
      return;
    }
    const wsKey = keyOf(null);
    if (sessions.current.has(wsKey)) { attach(wsKey); return; }
    const anyInWs = [...sessions.current.values()].find((s) => s.workspace === (getWorkspacePrefix() || 'default'));
    if (anyInWs) { attach(anyInWs.key); return; }
    await create(null);
  }, [attach, create]);

  useEffect(() => {
    bridge.terminal = {
      show,
      open: (sessionId) => show({ sessionId }),
      counts: (sessionId) => [...sessions.current.values()].filter((s) => s.sessionId === sessionId).length,
    };
    return () => { bridge.terminal = null; };
  }, [show]);

  // 面板激活：若当前工作区没有终端则自动启动一个。
  useEffect(() => {
    if (!active) return;
    const current = activeKey ? sessions.current.get(activeKey) : null;
    if (current && current.workspace === (getWorkspacePrefix() || 'default')) attach(current.key);
    else show({});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  // 切工作区：仅隐藏（保留缓冲与服务端会话），激活时切到该工作区的终端组。
  useEffect(() => {
    const onWs = () => {
      bodyRef.current?.replaceChildren();
      setActiveKey(null);
      if (activeRef.current) show({});
    };
    document.addEventListener('workspace-changed', onWs);
    return () => document.removeEventListener('workspace-changed', onWs);
  }, [show]);

  useEffect(() => {
    const body = bodyRef.current;
    if (!body) return;
    const ro = new ResizeObserver(() => {
      const session = activeKey ? sessions.current.get(activeKey) : null;
      if (session && activeRef.current && body.clientWidth > 0) resize(session);
    });
    ro.observe(body);
    return () => ro.disconnect();
  }, [activeKey, resize]);

  // 主题切换后同步终端配色。
  useEffect(() => {
    const id = requestAnimationFrame(() => {
      for (const session of sessions.current.values()) {
        try { session.term.options.theme = xtermTheme(); } catch { /* 旧版 API */ }
      }
    });
    return () => cancelAnimationFrame(id);
  }, [theme]);

  const workspace = getWorkspacePrefix() || 'default';
  const visible = [...sessions.current.values()].filter((s) => s.workspace === workspace);

  return (
    <>
      <PanelToolbar className="gap-1">
        <div role="tablist" aria-label="终端会话" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto [scrollbar-width:none]">
          {visible.map((s) => {
            const label = s.sessionId ? `会话 ${s.sessionId.slice(0, 6)}` : '工作区';
            return (
              <div key={s.key} role="tab" aria-selected={s.key === activeKey}
                className={cn('flex h-7 shrink-0 items-center rounded-md border border-transparent text-xs', s.key === activeKey ? 'border-border bg-background shadow-xs dark:bg-sidebar-accent' : 'text-muted-foreground hover:bg-sidebar-accent')}>
                <button type="button" className="flex items-center gap-1.5 pr-1 pl-2 font-mono text-[11px]" title={s.id} onClick={() => attach(s.key)}>
                  <SquareTerminalIcon className="size-3.5" />{label}<span className="text-muted-foreground">{(s.id || '?').slice(0, 4)}</span>
                </button>
                <button type="button" aria-label="关闭终端" className="mr-1 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => close(s.key)}>
                  <XIcon className="size-3" />
                </button>
              </div>
            );
          })}
        </div>
        <Button variant="ghost" size="xs" onClick={() => create(null)}><PlusIcon />新终端</Button>
      </PanelToolbar>
      <div className="relative min-h-0 flex-1 bg-background/40">
        <div ref={bodyRef} className="absolute inset-0" />
        {!visible.length && (
          <EmptyHint icon={SquareTerminalIcon} title="没有终端" className="absolute inset-0">
            点击「新终端」启动 shell，或在会话的附属行打开附属终端。
          </EmptyHint>
        )}
      </div>
    </>
  );
}
