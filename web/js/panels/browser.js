// 浏览器面板：多标签 + URL 栏 + 代理/直连双模式 + 每会话附属绑定。
// 默认「代理」：URL 由服务端 /api/proxy 经设备网络抓取（远程也能访问设备内网
// 页面），脚本剥离后以同源禁脚本 CSP 渲染；「直连」走本机 iframe。
// 附属于会话的标签持久化在 /api/sessions/{id}/attachments（url+mode），
// 重载页面后按会话恢复；工作区级标签不持久化。

import { h, cn, button, toast } from '../ui.js';
import { api, apiPath, apiPut } from '../api.js';

let tabSeq = 1;

function normalizeUrl(raw) {
  const value = String(raw || '').trim();
  if (!value) return '';
  if (/^https?:\/\//i.test(value)) return value;
  if (/^[\w.-]+\.[a-z]{2,}(\/|$|:)/i.test(value)) return `https://${value}`;
  return '';
}

function hostOf(url) {
  try { return new URL(url).host || url; } catch (_) { return url; }
}

export function createBrowserPane({ container, onClose }) {
  // tab: { id, url, mode: 'proxy'|'direct', sessionId|null }
  // sessionId=null 的工作区级标签只在当前页面生命周期存在；
  // 会话附属标签持久化到 attachments（url+mode），重开后按会话恢复。
  let tabs = [];
  let activeId = null;
  let savedTimer = 0;

  const strip = h('div', { class: 'browser-tabs', role: 'tablist', 'aria-label': '浏览器标签' });
  const urlInput = h('input', {
    'data-component': 'input', class: 'browser-url mono',
    placeholder: '输入网址，回车前往', spellcheck: 'false', 'aria-label': '网址',
    onkeydown: (e) => { if (e.key === 'Enter') goto(urlInput.value); },
  });
  const modeBtn = button('代理', {
    'data-variant': 'ghost', 'data-size': 'sm', 'aria-label': '切换代理/直连', title: '代理=经设备网络转发（可访问设备内网）；直连=本机 iframe',
    onclick: () => toggleMode(),
  });
  const backBtn = button('←', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '后退', title: '后退',
    onclick: () => { try { frame.contentWindow?.history.back(); } catch (_) { toast('跨源页面无法遥控后退'); } },
  });
  const reloadBtn = button('↻', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '重新加载', title: '重新加载',
    onclick: () => reload(),
  });
  const externalBtn = button('↗', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '在系统浏览器打开', title: '在系统浏览器打开',
    onclick: () => {
      const tab = activeTab();
      if (tab?.url) window.open(tab.url, '_blank', 'noopener');
    },
  });
  const closeBtn = button('✕', {
    'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '关闭浏览器面板',
    onclick: () => onClose?.(),
  });
  const frame = h('iframe', { class: 'browser-frame', title: '浏览器页面', hidden: true });
  const start = h('div', { class: 'browser-start' },
    h('p', { class: 'browser-start-title' }, '设备浏览器'),
    h('p', { class: 'text-ui-sm text-subtle' }, '输入网址，经运行设备网络转发（默认代理，可访问其内网页面）。'),
  );

  const activeTab = () => tabs.find((t) => t.id === activeId) || null;

  function renderStrip() {
    strip.replaceChildren();
    for (const tab of tabs) {
      strip.append(h('div', { class: cn('browser-tab', tab.id === activeId && 'is-active'), role: 'tab', 'aria-selected': String(tab.id === activeId) },
        h('button', {
          type: 'button', class: 'browser-tab-main', title: tab.url,
          onclick: () => activate(tab.id),
        }, hostOf(tab.url) || '空白页'),
        h('button', {
          type: 'button', class: 'browser-tab-close', 'aria-label': '关闭标签',
          onclick: () => closeTab(tab.id),
        }, '✕'),
      ));
    }
    strip.append(button('＋', {
      'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '新标签页',
      onclick: () => { newTab(); },
    }));
    const tab = activeTab();
    urlInput.value = tab?.url || '';
    modeBtn.textContent = tab?.mode === 'proxy' ? '代理' : '直连';
  }

  function newTab(url = '', mode = 'proxy', sessionId = null) {
    const tab = { id: tabSeq++, url, mode, sessionId };
    tabs.push(tab);
    activeId = tab.id;
    renderStrip();
    schedulePersist();
    if (url) load(tab);
    else showStart();
    return tab;
  }

  function closeTab(id) {
    const idx = tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    tabs = tabs.filter((t) => t.id !== id);
    schedulePersist();
    if (activeId === id) {
      const next = tabs[idx - 1] || tabs[idx] || null;
      if (next) activate(next.id);
      else { activeId = null; showStart(); renderStrip(); }
    } else renderStrip();
  }

  function activate(id) {
    activeId = id;
    const tab = activeTab();
    if (tab?.url) load(tab);
    else showStart();
    renderStrip();
  }

  function showStart() {
    frame.hidden = true;
    frame.removeAttribute('src');
    start.hidden = false;
  }

  function load(tab) {
    start.hidden = true;
    frame.hidden = false;
    frame.src = tab.mode === 'proxy'
      ? apiPath(`/api/proxy?url=${encodeURIComponent(tab.url)}`)
      : tab.url;
  }

  function goto(raw) {
    const url = normalizeUrl(raw);
    if (!url) { toast('请输入合法网址'); return; }
    const tab = activeTab() || newTab('', 'proxy', boundSessionId);
    tab.url = url;
    load(tab);
    renderStrip();
    schedulePersist();
  }

  function reload() {
    const tab = activeTab();
    if (tab?.url) load(tab);
  }

  function toggleMode() {
    const tab = activeTab();
    if (!tab) return;
    if (!tab.url) { toast('当前是空白页'); return; }
    tab.mode = tab.mode === 'proxy' ? 'direct' : 'proxy';
    load(tab);
    renderStrip();
    toast(tab.mode === 'proxy' ? '已切换代理模式（脚本已剥离）' : '已切换直连模式');
  }

  urlInput.addEventListener('focus', () => urlInput.select());

  /* ---------- 会话附属持久化 ---------- */
  let boundSessionId = null;

  // 300ms 防抖写回：url 输入连续跳转不放大写盘。
  function schedulePersist() {
    if (!boundSessionId) return;
    clearTimeout(savedTimer);
    savedTimer = setTimeout(persistAttachments, 300);
  }

  async function persistAttachments() {
    const sessionId = boundSessionId;
    if (!sessionId) return;
    const tabsOwned = tabs.filter((tab) => tab.sessionId === sessionId);
    try {
      await apiPut(`/api/sessions/${encodeURIComponent(sessionId)}/attachments`, {
        terminal_ids: [],
        browser_tabs: tabsOwned.map((tab) => ({ url: tab.url, mode: tab.mode })),
      });
    } catch (_) { /* 持久化失败不阻断浏览（404 = 会话已删） */ }
  }

  // 会话切换时恢复持久化标签；调用方（dock/main）传当前 sessionId。
  async function adoptSession(sessionId) {
    boundSessionId = sessionId;
    clearTimeout(savedTimer);
    if (!sessionId) return;
    try {
      const stored = await api(`/api/sessions/${encodeURIComponent(sessionId)}/attachments`);
      const owned = tabs.filter((tab) => tab.sessionId === sessionId);
      const restored = Array.isArray(stored?.browser_tabs) ? stored.browser_tabs : [];
      // 以持久化为准补齐缺的（内存优先保留未关闭的 URL）。
      const existing = new Set(owned.map((tab) => `${tab.mode} ${tab.url}`));
      for (const ref of restored) {
        if (!existing.has(`${ref.mode} ${ref.url}`)) {
          tabs.push({ id: tabSeq++, url: String(ref.url || ''), mode: ref.mode === 'direct' ? 'direct' : 'proxy', sessionId });
        }
      }
      // 切回该会话时聚焦其第一个标签；没有则空白。
      const first = tabs.find((tab) => tab.sessionId === sessionId);
      if (first) activate(first.id);
      else { activeId = null; showStart(); }
      renderStrip();
    } catch (_) { /* 会话不存在等：静默降级为无绑定视图 */ }
  }

  container.replaceChildren(
    h('div', { class: 'browser-toolbar' }, backBtn, urlInput, reloadBtn, modeBtn, externalBtn, closeBtn),
    strip,
    h('div', { class: 'browser-body' }, start, frame),
  );
  renderStrip();
  showStart();

  return {
    // 面板显示时调用：sessionId 提供时采用会话附属标签（重开先从 attachments 恢复），
    // 否则维持工作区级标签视图；无标签时开一个空白标签。
    async show(...args) {
      const options = typeof args[0] === 'object' && args[0] !== null ? args[0] : {};
      if (options.sessionId !== undefined) {
        const sessionId = options.sessionId || null;
        if (sessionId && sessionId !== boundSessionId) await adoptSession(sessionId);
        else if (!sessionId) { boundSessionId = null; renderStrip(); }
      }
      if (!tabs.length) newTab('', 'proxy', null);
    },
    open(sessionId) {
      boundSessionId = sessionId || null;
      if (sessionId) return adoptSession(sessionId);
      if (!tabs.length) newTab('', 'proxy', null);
      return Promise.resolve();
    },
    hide() {},
    get url() { return activeTab()?.url || ''; },
    counts(sessionId) {
      return { terms: 0, tabs: tabs.filter((tab) => tab.sessionId === sessionId).length };
    },
  };
}
