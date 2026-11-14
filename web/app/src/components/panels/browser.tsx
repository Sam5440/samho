// 浏览器面板：多标签 + URL 栏 + 代理/直连双模式 + 每会话附属绑定。
// 默认「代理」：URL 由服务端 /api/proxy 经设备网络抓取（远程也能访问设备内网页面），
// 脚本剥离后以禁脚本 CSP 渲染；「直连」走本机 iframe。
// 会话附属标签持久化在 /api/sessions/{id}/attachments（url+mode）；工作区级标签不持久化。

import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeftIcon, ExternalLinkIcon, GlobeIcon, PlusIcon, RotateCwIcon, ShieldIcon, XIcon, ZapIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Tip } from '@/components/ui/tooltip';
import { toast } from '@/components/ui/toaster';
import { api, apiPath, apiPut } from '@/lib/api';
import { bridge } from '@/lib/bridge';
import { setState } from '@/lib/store';
import { cn } from '@/lib/utils';

type Tab = { id: number; url: string; mode: 'proxy' | 'direct'; sessionId: string | null; nonce: number };
let tabSeq = 1;

function normalizeUrl(raw: string) {
  const value = String(raw || '').trim();
  if (!value) return '';
  if (/^https?:\/\//i.test(value)) return value;
  if (/^(localhost|\d{1,3}(\.\d{1,3}){3})(:\d+)?(\/|$)/i.test(value)) return `http://${value}`;
  if (/^[\w.-]+\.[a-z]{2,}(\/|$|:)/i.test(value)) return `https://${value}`;
  return '';
}

function hostOf(url: string) {
  try { return new URL(url).host || url; } catch { return url; }
}

export function BrowserPanel({ active }: { active: boolean }) {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [address, setAddress] = useState('');
  const boundSession = useRef<string | null>(null);
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;
  const frameRef = useRef<HTMLIFrameElement>(null);
  const persistTimer = useRef(0);
  const current = tabs.find((t) => t.id === activeId) || null;

  useEffect(() => { setAddress(current?.url || ''); }, [current?.id, current?.url]);
  useEffect(() => { setState((s) => ({ attachVersion: s.attachVersion + 1 })); }, [tabs.length]);

  // 300ms 防抖写回：url 连续跳转不放大写盘。
  const schedulePersist = useCallback(() => {
    const sessionId = boundSession.current;
    if (!sessionId) return;
    clearTimeout(persistTimer.current);
    persistTimer.current = window.setTimeout(async () => {
      const owned = tabsRef.current.filter((tab) => tab.sessionId === sessionId);
      try {
        await apiPut(`/api/sessions/${encodeURIComponent(sessionId)}/attachments`, {
          terminal_ids: [],
          browser_tabs: owned.map((tab) => ({ url: tab.url, mode: tab.mode })),
        });
      } catch { /* 持久化失败不阻断浏览（404 = 会话已删） */ }
    }, 300);
  }, []);

  const update = (next: Tab[], nextActive?: number | null) => {
    setTabs(next);
    tabsRef.current = next;
    if (nextActive !== undefined) setActiveId(nextActive);
    schedulePersist();
  };

  const newTab = (url = '', sessionId: string | null = boundSession.current) => {
    const tab: Tab = { id: tabSeq++, url, mode: 'proxy', sessionId, nonce: 0 };
    update([...tabsRef.current, tab], tab.id);
    return tab;
  };

  const closeTab = (id: number) => {
    const list = tabsRef.current;
    const idx = list.findIndex((t) => t.id === id);
    const next = list.filter((t) => t.id !== id);
    update(next, activeId === id ? (next[idx - 1] || next[idx] || null)?.id ?? null : activeId);
  };

  const goto = (raw: string) => {
    const url = normalizeUrl(raw);
    if (!url) { toast('请输入合法网址', 'error'); return; }
    if (!current) { newTab(url); return; }
    update(tabsRef.current.map((t) => (t.id === current.id ? { ...t, url, nonce: t.nonce + 1 } : t)));
  };

  const toggleMode = () => {
    if (!current) return;
    if (!current.url) { toast('当前是空白页'); return; }
    const mode = current.mode === 'proxy' ? 'direct' : 'proxy';
    update(tabsRef.current.map((t) => (t.id === current.id ? { ...t, mode, nonce: t.nonce + 1 } : t)));
    toast(mode === 'proxy' ? '已切换代理模式（脚本已剥离）' : '已切换直连模式');
  };

  const reload = () => {
    if (!current?.url) return;
    update(tabsRef.current.map((t) => (t.id === current.id ? { ...t, nonce: t.nonce + 1 } : t)));
  };

  // 会话切换时恢复持久化标签。
  const adoptSession = useCallback(async (sessionId: string) => {
    boundSession.current = sessionId;
    clearTimeout(persistTimer.current);
    try {
      const stored = await api(`/api/sessions/${encodeURIComponent(sessionId)}/attachments`);
      const restored: { url: string; mode: string }[] = Array.isArray(stored?.browser_tabs) ? stored.browser_tabs : [];
      const list = [...tabsRef.current];
      const existing = new Set(list.filter((t) => t.sessionId === sessionId).map((t) => `${t.mode} ${t.url}`));
      for (const ref of restored) {
        const mode = ref.mode === 'direct' ? 'direct' : 'proxy';
        if (!existing.has(`${mode} ${ref.url}`)) list.push({ id: tabSeq++, url: String(ref.url || ''), mode, sessionId, nonce: 0 });
      }
      setTabs(list);
      tabsRef.current = list;
      const first = list.find((t) => t.sessionId === sessionId);
      if (first) setActiveId(first.id);
      else newTab('', sessionId);
    } catch { /* 会话不存在等：静默降级为无绑定视图 */ }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const show = useCallback(async ({ sessionId }: { sessionId?: string | null } = {}) => {
    if (sessionId !== undefined) {
      if (sessionId && sessionId !== boundSession.current) await adoptSession(sessionId);
      else if (!sessionId) boundSession.current = null;
    }
    if (!tabsRef.current.length) newTab('', null);
  }, [adoptSession]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    bridge.browser = {
      show,
      open: (sessionId) => adoptSession(sessionId),
      counts: (sessionId) => tabsRef.current.filter((t) => t.sessionId === sessionId).length,
    };
    return () => { bridge.browser = null; };
  }, [show, adoptSession]);

  useEffect(() => { if (active && !tabsRef.current.length) newTab('', null); }, [active]); // eslint-disable-line react-hooks/exhaustive-deps

  const src = current?.url
    ? (current.mode === 'proxy' ? apiPath(`/api/proxy?url=${encodeURIComponent(current.url)}`) : current.url)
    : '';

  return (
    <>
      <div className="flex h-10 shrink-0 items-center gap-1 border-b px-2">
        <Tip text="后退">
          <Button variant="ghost" size="icon-xs" aria-label="后退"
            onClick={() => { try { frameRef.current?.contentWindow?.history.back(); } catch { toast('跨源页面无法遥控后退'); } }}>
            <ArrowLeftIcon />
          </Button>
        </Tip>
        <Tip text="重新加载"><Button variant="ghost" size="icon-xs" aria-label="重新加载" onClick={reload}><RotateCwIcon /></Button></Tip>
        <form className="relative min-w-0 flex-1" onSubmit={(e) => { e.preventDefault(); goto(address); }}>
          <GlobeIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onFocus={(e) => e.target.select()}
            placeholder="输入网址，回车前往"
            spellCheck={false}
            aria-label="网址"
            className="h-7 pl-8 font-mono text-xs"
          />
        </form>
        <Tip text={current?.mode === 'direct' ? '直连：本机 iframe（点击切换为代理）' : '代理：经设备网络转发，可访问设备内网（点击切换为直连）'}>
          <Button variant="ghost" size="xs" aria-label="切换代理/直连" onClick={toggleMode}
            className={cn('font-normal', current?.mode === 'direct' ? 'text-warning' : 'text-muted-foreground')}>
            {current?.mode === 'direct' ? <ZapIcon /> : <ShieldIcon />}{current?.mode === 'direct' ? '直连' : '代理'}
          </Button>
        </Tip>
        <Tip text="在系统浏览器打开">
          <Button variant="ghost" size="icon-xs" aria-label="在系统浏览器打开" onClick={() => current?.url && window.open(current.url, '_blank', 'noopener')}>
            <ExternalLinkIcon />
          </Button>
        </Tip>
      </div>
      <div role="tablist" aria-label="浏览器标签" className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b px-2 [scrollbar-width:none]">
        {tabs.map((tab) => (
          <div key={tab.id} role="tab" aria-selected={tab.id === activeId}
            className={cn('flex h-6 max-w-40 shrink-0 items-center rounded-md border border-transparent text-xs', tab.id === activeId ? 'border-border bg-background shadow-xs dark:bg-sidebar-accent' : 'text-muted-foreground hover:bg-sidebar-accent')}>
            <button type="button" title={tab.url} onClick={() => setActiveId(tab.id)} className="min-w-0 truncate pr-1 pl-2">
              {tab.sessionId && <span className="mr-1 inline-block size-1.5 rounded-full bg-brand align-middle" title="会话附属" />}
              {hostOf(tab.url) || '空白页'}
            </button>
            <button type="button" aria-label="关闭标签" onClick={() => closeTab(tab.id)} className="mr-0.5 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground">
              <XIcon className="size-3" />
            </button>
          </div>
        ))}
        <Tip text="新标签页"><Button variant="ghost" size="icon-xs" className="size-6" aria-label="新标签页" onClick={() => newTab()}><PlusIcon /></Button></Tip>
      </div>
      <div className="relative min-h-0 flex-1 bg-background">
        {src ? (
          <iframe key={`${current?.id}:${current?.nonce}:${current?.mode}`} ref={frameRef} title="浏览器页面" src={src} className="absolute inset-0 size-full border-0 bg-white" />
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 px-8 text-center">
            <GlobeIcon className="mb-1 size-6 text-muted-foreground/60" />
            <p className="text-sm font-medium">设备浏览器</p>
            <p className="text-xs leading-relaxed text-muted-foreground">输入网址，经运行设备网络转发（默认代理，可访问其内网页面）。</p>
          </div>
        )}
      </div>
    </>
  );
}
