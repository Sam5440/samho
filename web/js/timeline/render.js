// 时间线渲染：按 row.key 复用 DOM；运行中任务在尾部渲染 live turn；
// auto-scroll 采用 pin/unpin 单一事实源（向上滚动即 unpin）。

import { h, cn, button, spinner, badge } from '../ui.js';
import { renderMarkdown } from '../markdown.js';
import { projectTimeline } from './projection.js';
import { toolCard, reasoningBlock } from './tools.js';

const NEAR_BOTTOM = 80;

function signature(row) {
  switch (row.type) {
    case 'text': return `${row.text.length}:${row.text.slice(-40)}:${row.model || ''}`;
    case 'reasoning': return `${row.text.length}:${row.text.slice(-40)}:${row.completed ? 'd' : 'r'}`;
    case 'tool': return `${row.status}:${(row.result || '').length}:${JSON.stringify(row.args).length}`;
    case 'user': return `${row.text.length}:${row.text.slice(-40)}`;
    default: return row.text || '';
  }
}

function turnDivider(turn) {
  return h('div', { class: 'turn-divider', role: 'separator' },
    h('span', { class: 'turn-divider-label' }, `第 ${turn} 轮`));
}

function userCard(row) {
  return h('article', { class: 'msg-user' },
    h('div', { class: 'msg-user-bubble' }, row.text));
}

function textBlock(row, live = false) {
  const article = h('article', { class: 'msg-assistant' });
  article.append(h('div', { class: 'md-body', html: renderMarkdown(row.text) }));
  const meta = h('div', { class: 'msg-meta' });
  if (row.model) meta.append(badge(row.model, 'outline', true));
  if (live) meta.append(badge('流式输出', 'info'));
  if (meta.childElementCount) article.append(meta);
  return article;
}

function noteBlock(row) {
  return h('div', { class: cn('msg-note', row.tone === 'error' && 'is-error') }, row.text);
}

function rowElement(row, live = false) {
  switch (row.type) {
    case 'turn': return turnDivider(row.turn);
    case 'user': return userCard(row);
    case 'text': return textBlock(row, live);
    case 'reasoning': return reasoningBlock({
      text: row.text, running: !row.completed,
      startedAt: row.completed ? (row.durationMs || 0) : 0,
    });
    case 'tool': return toolCard(row);
    case 'note': return noteBlock(row);
    default: return h('div', {}, row.text || '');
  }
}

// live turn：流式文本 + 流式工具调用（未配对结果时显示 running）。
// 容器与流式文章跨帧复用（否则 CSS 动画每帧重启）——BorderBeamCard 需要持久元素。
let liveWrap = null;
let liveArticle = null;

function liveTurn(live, reuse = liveWrap) {
  const wrap = reuse || h('section', { class: 'live-turn', 'aria-live': 'polite' });
  wrap.replaceChildren();
  if (live.turn) wrap.append(h('div', { class: 'turn-divider' }, h('span', { class: 'turn-divider-label' }, `第 ${live.turn} 轮${live.attempt > 1 ? ` · 重试 ${live.attempt}` : ''}`)));
  if (live.reasoning && live.reasoning.trim()) {
    wrap.append(reasoningBlock({ text: live.reasoning, running: true, startedAt: live.startedAt }));
  }
  liveArticle = null;
  if (live.text && live.text.trim()) {
    const article = (reuse && wrap.querySelector('[data-beam="mono"]')) || h('article', { class: 'msg-assistant', 'data-beam': 'mono' });
    liveArticle = article;
    article.replaceChildren(h('div', { class: 'md-body', html: renderMarkdown(live.text) }));
    const meta = h('div', { class: 'msg-meta' });
    if (live.modelId) meta.append(badge(live.modelId, 'outline', true));
    meta.append(h('span', { class: 'msg-meta-inline' }, spinner('0.7rem')), badge('正在生成', 'info'));
    article.append(meta);
    wrap.append(article);
  }
  const shown = new Set();
  for (const call of live.toolCalls || []) {
    const id = String(call?.id || '');
    if (!id || shown.has(id)) continue;
    shown.add(id);
    const snapshot = live.tools?.get(id);
    let args = {};
    let input = '';
    const rawArgs = call?.function?.arguments;
    if (typeof rawArgs === 'string') { input = rawArgs; try { args = JSON.parse(rawArgs); } catch (_) { args = { _raw: rawArgs }; } }
    else if (rawArgs) { args = rawArgs; input = JSON.stringify(rawArgs, null, 2); }
    const status = snapshot ? (snapshot.status === 'done' ? 'completed' : snapshot.status === 'error' ? 'error' : 'running') : 'running';
    wrap.append(toolCard({
      key: `t:${id}`, type: 'tool', name: call?.function?.name || snapshot?.name || 'tool', callId: id,
      args, input, status, result: snapshot?.result || '',
      latencyMs: snapshot?.latencyMs,
    }));
  }
  for (const [id, snapshot] of live.tools || []) {
    if (shown.has(id)) continue;
    shown.add(id);
    wrap.append(toolCard({
      key: `t:${id}`, type: 'tool', name: snapshot.name, callId: id,
      args: snapshot.args, input: snapshot.input, status: snapshot.status === 'done' ? 'completed' : snapshot.status === 'error' ? 'error' : 'running',
      result: snapshot.result, latencyMs: snapshot.latencyMs,
    }));
  }
  if (!wrap.childElementCount) {
    wrap.append(h('div', { class: 'live-waiting' }, spinner(), h('span', { class: 'shimmer-text' }, '智能体正在执行任务…')));
  }
  return wrap;
}

export function createTimeline(container) {
  const inner = h('div', { class: 'timeline-inner' });
  const jump = button('↓ 回到底部', { 'data-variant': 'secondary', 'data-size': 'sm', class: 'jump-to-bottom', hidden: true });
  container.append(inner, jump);
  let pinned = true;
  const rendered = new Map(); // key -> { sig, el }

  const isPinned = () => container.scrollTop + container.clientHeight >= container.scrollHeight - NEAR_BOTTOM;
  container.addEventListener('scroll', () => {
    pinned = isPinned();
    jump.hidden = pinned;
  }, { passive: true });
  // 选中文本时不自动跟随（OpenCode interaction 移植）。
  document.addEventListener('selectionchange', () => {
    const selection = document.getSelection && document.getSelection();
    if (selection && !selection.isCollapsed && container.contains(selection.anchorNode)) pinned = false;
  });
  jump.addEventListener('click', () => {
    pinned = true;
    container.scrollTop = container.scrollHeight;
    jump.hidden = true;
  });
  const follow = (force) => {
    if (force) pinned = true;
    if (pinned) container.scrollTop = container.scrollHeight;
    jump.hidden = pinned;
  };

  function sync(messages, live = null) {
    const rows = projectTimeline(messages, live);
    const keep = new Set();
    const fragment = document.createDocumentFragment();
    for (const row of rows) {
      keep.add(row.key);
      const sig = signature(row);
      const prev = rendered.get(row.key);
      if (prev && prev.sig === sig && prev.el.parentNode === inner) {
        fragment.append(prev.el);
        continue;
      }
      const el = rowElement(row);
      rendered.set(row.key, { sig, el });
      fragment.append(el);
    }
    for (const [key, entry] of rendered) {
      if (!keep.has(key)) { entry.el.remove(); rendered.delete(key); }
    }
    inner.replaceChildren(fragment);
    if (live && live.running) {
      liveWrap = liveTurn(live, liveWrap);
      inner.append(liveWrap);
    } else if (liveWrap) {
      liveWrap.remove();
      liveWrap = null;
    }
    follow();
    return rows.length;
  }

  return {
    sync,
    reset() {
      rendered.clear();
      inner.replaceChildren();
      liveWrap = null;
      liveArticle = null;
    },
    pin() { follow(true); },
  };
}
