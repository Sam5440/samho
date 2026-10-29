// 浏览器面板：多标签 + URL 栏 + 代理/直连双模式。
// 默认「代理」：URL 由服务端 /api/proxy 经设备网络抓取（远程也能访问设备内网
// 页面），脚本剥离后以同源禁脚本 CSP 渲染；「直连」走本机 iframe。

import { h, cn, button, toast } from '../ui.js';
import { apiPath } from '../api.js';

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
  let tabs = [];
  let activeId = null;

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

  function newTab(url = '', mode = 'proxy') {
    const tab = { id: tabSeq++, url, mode };
    tabs.push(tab);
    activeId = tab.id;
    renderStrip();
    if (url) load(tab);
    else showStart();
    return tab;
  }

  function closeTab(id) {
    const idx = tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    tabs = tabs.filter((t) => t.id !== id);
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
    const tab = activeTab() || newTab();
    tab.url = url;
    load(tab);
    renderStrip();
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

  container.replaceChildren(
    h('div', { class: 'browser-toolbar' }, backBtn, urlInput, reloadBtn, modeBtn, externalBtn, closeBtn),
    strip,
    h('div', { class: 'browser-body' }, start, frame),
  );
  renderStrip();
  showStart();

  return {
    // 面板显示时调用：无标签则开一个空白代理标签。
    open(url = '') {
      if (!tabs.length) newTab(url);
      else if (url) goto(url);
    },
    show() {
      if (!tabs.length) newTab('');
    },
    hide() {},
    get url() { return activeTab()?.url || ''; },
  };
}
