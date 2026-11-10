// 轨迹面板渲染（ZCode ModelTrajectoryPane 移植）：
// CallCard（sticky 摘要 + 输入/输出 section + error 块）+ 按角色展开 + 搜索定位。

import { h, cn, button, badge, spinner, openPopover, bindTooltip } from '../ui.js';
import { rollingNumber } from '../motion.js';
import { projectTrajectory, conversationDelta } from '../timeline/projection.js';
import { createExpansion, TRAJECTORY_ROLES, roleLabel } from './expansion.js';

const OMITTED = 'content omitted';
const isOmitted = (text) => typeof text === 'string' && text.includes(OMITTED);

const ROLE_TEXT = {
  system: 'SYSTEM', user: 'USER', assistant: 'ASSISTANT',
  tool: 'TOOL', developer: 'SYSTEM',
};

function escapeReg(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// 归一化匹配（ZCode ModelTrajectorySearch 简化版）：空白归一 + 小写。
function matches(text, query) {
  if (!query) return false;
  return String(text || '').replace(/\s+/g, ' ').toLowerCase().includes(query);
}

function highlightedPre(text, query) {
  const pre = h('pre', { class: 'traj-pre mono' });
  const raw = String(text ?? '');
  if (!query) { pre.textContent = raw; return pre; }
  const lower = raw.replace(/\s+/g, ' ').toLowerCase();
  const hit = lower.indexOf(query);
  if (hit < 0) { pre.textContent = raw; return pre; }
  // 逐字符推进原文本，把归一化命中的区间映射回原文。
  let norm = 0;
  const map = []; // normIndex -> srcIndex
  for (let src = 0; src < raw.length; src++) {
    map[norm] = src;
    const ch = raw[src];
    norm += /\s/.test(ch) ? (map[norm - 1] >= 0 && raw[src - 1] && !/\s/.test(raw[src - 1]) ? 1 : 0) : 1;
    if (/\s/.test(ch) && raw[src - 1] && /\s/.test(raw[src - 1])) map[norm] = src;
  }
  map[norm] = raw.length;
  // 安全渲染：全部内容走文本节点，命中区间包进 mark 元素（textContent 赋值），
  // 不拼接任何标记字符串——trace 正文（模型/工具/用户输入）不可信，旧实现
  // 把命中区间外的原文拼进标记字符串会让浏览器解析其中的元素。
  let cursor = 0;
  let at = hit;
  while (at >= 0 && at < lower.length) {
    const startSrc = map[at] ?? cursor;
    const endSrc = map[at + query.length] ?? raw.length;
    if (startSrc > cursor) pre.append(raw.slice(cursor, startSrc));
    const mark = document.createElement('mark');
    mark.textContent = raw.slice(startSrc, endSrc);
    pre.append(mark);
    cursor = endSrc;
    at = lower.indexOf(query, at + Math.max(1, query.length));
  }
  if (cursor < raw.length) pre.append(raw.slice(cursor));
  return pre;
}

function messageRow({ role, text, expansionKey, visualRole, expansion, query, pills = [] }) {
  const open = expansion.isOpen(expansionKey, visualRole);
  const row = h('div', { class: 'traj-msg', 'data-role': visualRole });
  const label = h('div', { class: 'traj-role mono' }, ROLE_TEXT[role] || visualRole.toUpperCase());
  const body = h('div', { class: 'traj-body min-w-0' });
  const previewText = String(text || '').replace(/\s+/g, ' ').trim();
  const trigger = h('button', {
    type: 'button',
    class: 'traj-msg-trigger',
    'aria-expanded': String(open),
  },
    h('span', { class: 'traj-direction' }, pills.length ? '' : '内容'),
    ...pills,
    h('span', { class: 'traj-preview min-w-0' }, previewText.slice(0, 160) || '—'),
    h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, '▾'),
  );
  const content = h('div', { class: 'traj-msg-content', hidden: !open });
  if (isOmitted(text)) {
    content.append(h('div', { class: 'traj-omitted' },
      badge('内容开关关闭', 'warning'),
      h('span', { class: 'text-ui-sm text-muted' }, '运行时未记录消息内容（log_content）。打开后可在此查看完整输入输出。')));
  } else {
    content.append(highlightedPre(text, query));
  }
  trigger.addEventListener('click', () => {
    expansion.toggle(expansionKey, visualRole);
    const nowOpen = expansion.isOpen(expansionKey, visualRole);
    trigger.setAttribute('aria-expanded', String(nowOpen));
    content.hidden = !nowOpen;
    row.dataset.open = String(nowOpen);
  });
  body.append(trigger, content);
  row.append(label, body);
  row.dataset.open = String(open);
  return row;
}

function toolRow({ name, callId, input, output, isError, key, expansion, query }) {
  const open = expansion.isOpen(key, isError ? 'tool-result' : 'tool-call');
  const row = h('div', { class: 'traj-msg traj-tool', 'data-role': 'tool-call', ...(isError ? { 'data-error': 'true' } : {}) });
  const label = h('div', { class: 'traj-role mono' }, 'TOOL');
  const body = h('div', { class: 'traj-body min-w-0' });
  const namePill = h('span', { class: 'traj-pill mono' }, name || 'tool');
  const idPill = callId ? h('span', { class: 'traj-pill mono' }, String(callId).replace(/^call_/, '')) : null;
  const trigger = h('button', { type: 'button', class: 'traj-msg-trigger', 'aria-expanded': String(open) },
    h('span', { class: cn('traj-direction', isError && 'is-error') }, isError ? '✕ 错误' : '输出'),
    namePill, idPill,
    h('span', { class: 'traj-preview min-w-0' }, String(output ?? input ?? '').replace(/\s+/g, ' ').trim().slice(0, 160) || '—'),
    h('span', { class: 'traj-chevron', 'aria-hidden': 'true' }, '▾'),
  );
  const content = h('div', { class: 'traj-msg-content', hidden: !open });
  content.append(h('div', { class: 'traj-io-label' }, '输入'));
  content.append(highlightedPre(input, query));
  if (output !== undefined && output !== null) {
    content.append(h('div', { class: 'traj-io-label' }, isError ? '错误' : '结果'));
    content.append(highlightedPre(output, query));
  }
  trigger.addEventListener('click', () => {
    expansion.toggle(key, isError ? 'tool-result' : 'tool-call');
    const nowOpen = expansion.isOpen(key, isError ? 'tool-result' : 'tool-call');
    trigger.setAttribute('aria-expanded', String(nowOpen));
    content.hidden = !nowOpen;
    row.dataset.open = String(nowOpen);
  });
  body.append(trigger, content);
  row.append(label, body);
  row.dataset.open = String(open);
  return row;
}

function callCard({ call, index, previous, expansion, query }) {
  const card = h('article', { class: 'traj-card', 'data-trajectory-call': '' });
  const usage = call.usage || {};
  const finishVariant = call.error ? 'destructive' : call.finishReason === 'tool_calls' ? 'info' : 'success';
  const summary = h('div', { class: 'traj-card-summary' },
    h('span', { class: 'traj-index mono' }, String(index + 1).padStart(2, '0')),
    h('span', { class: 'traj-model mono' }, call.modelId || '—'),
    call.finishReason ? badge(call.finishReason, finishVariant, true) : null,
    call.error ? badge('错误', 'destructive') : null,
  );
  const meta = h('span', { class: 'traj-meta mono' });
  // token 计数走 RollingNumber（CallCard 每次重挂载 → 首帧直接落位）。
  const pushCount = (label, value) => {
    if (meta.childElementCount) meta.append(' · ');
    const span = h('span', { class: 'roll-num' });
    meta.append(`${label} `, span);
    rollingNumber(span, value);
  };
  if (usage.input_tokens !== undefined) pushCount('IN', Number(usage.input_tokens));
  if (usage.output_tokens !== undefined) pushCount('OUT', Number(usage.output_tokens));
  if (meta.childElementCount) meta.append(' · ');
  meta.append(`${call.latencyMs ? `${(call.latencyMs / 1000).toFixed(1)}s · ` : ''}${new Date(call.startedAt).toLocaleTimeString('zh-CN', { hour12: false })}`);
  summary.append(meta);

  const delta = conversationDelta(previous ? previous.request : null, call.request);
  const inputSection = h('div', { class: 'traj-section' }, h('div', { class: 'traj-section-title' }, '输入', badge(`${delta.length} 条新增`, 'outline')));
  if (!delta.length) {
    inputSection.append(h('div', { class: 'traj-empty-note' }, '与上一次调用相比没有新增消息。'));
  }
  delta.forEach((message, i) => {
    const role = String(message.role || 'unknown').toLowerCase();
    const text = typeof message.content === 'string' ? message.content : JSON.stringify(message.content, null, 2);
    const visualRole = role === 'user' ? 'user' : role === 'assistant' ? 'assistant' : role === 'tool' ? 'tool-result' : 'system';
    inputSection.append(messageRow({
      role, text, query,
      expansionKey: `${call.id}:input:${i}`,
      visualRole, expansion,
      pills: role === 'tool' && message.tool_call_id ? [h('span', { class: 'traj-pill mono' }, String(message.tool_call_id).replace(/^call_/, ''))] : [],
    }));
  });

  const outputSection = h('div', { class: 'traj-section' }, h('div', { class: 'traj-section-title' }, '输出'));
  const output = call.output || {};
  const reasoning = typeof output.reasoning_content === 'string' ? output.reasoning_content : '';
  if (reasoning.trim()) {
    outputSection.append(messageRow({
      role: 'assistant', text: reasoning, query,
      expansionKey: `${call.id}:output:reasoning`,
      visualRole: 'reasoning', expansion,
    }));
  }
  const text = typeof output.content === 'string' ? output.content : '';
  if (text.trim() || (!call.error && !reasoning.trim() && !Array.isArray(output.tool_calls))) {
    outputSection.append(messageRow({
      role: 'assistant', text: text || (call.output ? '' : ''), query,
      expansionKey: `${call.id}:output:text`,
      visualRole: 'assistant', expansion,
    }));
  }
  const calls = Array.isArray(output.tool_calls) ? output.tool_calls : [];
  calls.forEach((toolCall, i) => {
    outputSection.append(toolRow({
      name: toolCall?.function?.name,
      callId: toolCall?.id,
      input: typeof toolCall?.function?.arguments === 'string' ? toolCall.function.arguments : JSON.stringify(toolCall?.function?.arguments ?? {}, null, 2),
      output: undefined,
      isError: false,
      key: `${call.id}:output:tool-call:${i}`,
      expansion, query,
    }));
  });
  if (call.error) {
    outputSection.append(h('div', { class: 'traj-error-block' },
      h('div', { class: 'traj-error-title' }, '调用失败'),
      h('pre', { class: 'traj-pre mono' }, call.error)));
  }
  if (!outputSection.querySelector('.traj-msg, .traj-error-block') && call.error) {
    // 已有 error 块，无需占位。
  } else if (!outputSection.querySelector('.traj-msg, .traj-error-block')) {
    outputSection.append(h('div', { class: 'traj-empty-note' }, '该调用没有可展示的输出记录。'));
  }

  card.append(summary, inputSection, outputSection);
  return card;
}

export function createTrajectoryPanel(container, { getExportUrl } = {}) {
  const expansion = createExpansion();
  let events = [];
  let query = '';
  let loading = false;

  const summary = h('div', { class: 'traj-summary' });
  // 汇总数值跨渲染持久（同一元素重挂 → 滚柱动画连续）。
  const rollCalls = h('span', { class: 'roll-num mono' });
  const rollIn = h('span', { class: 'roll-num mono' });
  const rollOut = h('span', { class: 'roll-num mono' });
  const searchInput = h('input', {
    'data-component': 'input',
    placeholder: '搜索轨迹内容…',
    spellcheck: 'false',
  });
  const list = h('div', { class: 'traj-list min-h-0' });
  const placeholder = h('div', { class: 'traj-placeholder' },
    h('div', { class: 'traj-placeholder-mark' }, '◈'),
    h('span', {}, '等待执行信号'),
    h('span', { class: 'text-ui-sm text-muted' }, '从任务到结果，完整记录每一次模型调用。'));

  // 增量渲染事实源：已渲染调用卡身份 + 当时的搜索词。同前缀新调用直接
  // 追加卡片，避免每个进度帧/SSE 事件整表重建（长轨迹的主要开销来源）。
  let renderedIds = [];
  let renderedQuery = '';

  const eventKey = (event) => `${event?.trace_id}/${event?.sequence}/${event?.span_id}/${event?.kind}`;
  const seenEvents = new Set();

  function markHit(card, call) {
    if (!query) return;
    const hit = matches(call.modelId, query)
      || (call.request || []).some((m) => matches(typeof m.content === 'string' ? m.content : JSON.stringify(m.content), query))
      || matches(call.output?.content, query)
      || (call.output?.tool_calls || []).some((t) => matches(t?.function?.name, query) || matches(t?.function?.arguments, query))
      || matches(call.error, query);
    card.classList.toggle('is-hit', hit);
  }

  function buildCard(call, index, previous) {
    const card = callCard({ call, index, previous, expansion, query });
    markHit(card, call);
    return card;
  }

  function updateSummary(calls) {
    if (!calls.length) { summary.replaceChildren(); return; }
    const totalIn = calls.reduce((sum, c) => sum + Number(c.usage?.input_tokens || 0), 0);
    const totalOut = calls.reduce((sum, c) => sum + Number(c.usage?.output_tokens || 0), 0);
    const models = [...new Set(calls.map((c) => c.modelId).filter(Boolean))];
    summary.replaceChildren(
      h('span', {}, rollCalls, ' 次调用'),
      h('span', { class: 'mono' }, 'IN ', rollIn, ' · OUT ', rollOut),
      ...models.map((model) => badge(model, 'outline', true)),
    );
    rollingNumber(rollCalls, calls.length);
    rollingNumber(rollIn, totalIn);
    rollingNumber(rollOut, totalOut);
  }

  function renderEmptyState() {
    if (!events.length) { list.append(placeholder); return; }
    // 有执行事件但没有任何模型调用（如任务在路由后立即失败）。
    const kinds = new Map();
    for (const event of events) kinds.set(event.kind, (kinds.get(event.kind) || 0) + 1);
    list.append(h('div', { class: 'traj-placeholder' },
      h('div', { class: 'traj-placeholder-mark' }, '◈'),
      h('span', {}, '本次执行没有产生模型调用'),
      h('span', { class: 'text-ui-sm text-muted' }, '任务在模型调用前结束，仅有执行事件：'),
      h('div', { class: 'traj-placeholder-badges' },
        ...Array.from(kinds, ([kind, count]) => badge(`${kind} ×${count}`, 'outline', true)))));
  }

  function render(force = false) {
    const calls = projectTrajectory(events);
    if (!calls.length) {
      list.replaceChildren();
      renderedIds = [];
      renderedQuery = query;
      updateSummary(calls);
      renderEmptyState();
      return;
    }
    updateSummary(calls);
    const canAppend = !force
      && query === renderedQuery
      && calls.length > renderedIds.length
      && renderedIds.every((id, i) => calls[i]?.id === id);
    if (canAppend) {
      let previous = calls[renderedIds.length - 1] || null;
      for (let i = renderedIds.length; i < calls.length; i++) {
        list.append(buildCard(calls[i], i, previous));
        renderedIds.push(calls[i].id);
        previous = calls[i];
      }
      return;
    }
    list.replaceChildren();
    renderedIds = [];
    let previous = null;
    calls.forEach((call, index) => {
      list.append(buildCard(call, index, previous));
      renderedIds.push(call.id);
      previous = call;
    });
    renderedQuery = query;
  }

  // 只重建末卡：model.response / model.error 只修改最后一个调用对象。
  function refreshTail() {
    const calls = projectTrajectory(events);
    if (!calls.length
      || calls.length !== renderedIds.length
      || renderedIds.some((id, i) => calls[i]?.id !== id)) {
      return render();
    }
    const last = list.lastElementChild;
    if (!last) return render(true);
    const previous = calls.length > 1 ? calls[calls.length - 2] : null;
    last.replaceWith(buildCard(calls[calls.length - 1], calls.length - 1, previous));
  }

  searchInput.addEventListener('input', () => {
    query = searchInput.value.trim().toLowerCase();
    render(true);
  });

  function menu(anchor) {
    const panel = h('div', { class: 'expansion-menu' });
    for (const role of TRAJECTORY_ROLES) {
      const row = h('label', { class: 'expansion-row' },
        h('span', {}, roleLabel(role)));
      const sw = h('button', {
        type: 'button', role: 'switch', 'data-component': 'switch',
        'data-state': expansion.getRole(role) ? 'checked' : 'unchecked',
        'aria-checked': String(expansion.getRole(role)),
      });
      sw.addEventListener('click', () => {
        const next = !expansion.getRole(role);
        expansion.setRole(role, next);
        sw.dataset.state = next ? 'checked' : 'unchecked';
        sw.setAttribute('aria-checked', String(next));
        render(true);
      });
      row.append(sw);
      panel.append(row);
    }
    openPopover(anchor, panel, { align: 'end' });
  }

  const header = h('div', { class: 'traj-header' },
    h('div', { class: 'traj-header-row' },
      h('strong', {}, '模型轨迹'),
      h('span', { class: 'traj-header-actions' },
        button('全部展开', { 'data-variant': 'ghost', 'data-size': 'sm', onclick: (e) => { TRAJECTORY_ROLES.forEach((r) => expansion.setRole(r, true)); render(true); } }),
        button('全部收起', { 'data-variant': 'ghost', 'data-size': 'sm', onclick: () => { TRAJECTORY_ROLES.forEach((r) => expansion.setRole(r, false)); render(true); } }),
        button('⚙', { 'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '按角色设置展开', onclick: (e) => menu(e.currentTarget) }),
        getExportUrl ? button('导出', {
          'data-variant': 'outline', 'data-size': 'sm',
          onclick: () => { const url = getExportUrl(); if (url) window.open(url, '_blank', 'noopener'); },
        }) : null,
      ),
    ),
    h('div', { class: 'traj-search' }, searchInput),
  );

  container.append(header, summary, list);
  render();

  return {
    setEvents(next) {
      events = next || [];
      seenEvents.clear();
      for (const event of events) seenEvents.add(eventKey(event));
      render(true);
    },
    // 增量追加（SSE / 分页补齐）：身份去重；序号回退（补历史页）时整体重排。
    appendEvents(incoming) {
      const fresh = [];
      for (const event of incoming || []) {
        if (!event || typeof event !== 'object' || !event.kind) continue;
        const key = eventKey(event);
        if (seenEvents.has(key)) continue;
        seenEvents.add(key);
        fresh.push(event);
      }
      if (!fresh.length) return;
      const prevLast = events.length ? events[events.length - 1] : null;
      const outOfOrder = prevLast && Number(fresh[0].sequence || 0) < Number(prevLast.sequence || 0);
      events.push(...fresh);
      if (outOfOrder) {
        events.sort((a, b) => Number(a.sequence || 0) - Number(b.sequence || 0));
        render(true);
        return;
      }
      // 新调用 → 追加卡片；response/error 只改末卡 → 就地重建末卡。
      if (fresh.some((event) => event.kind === 'model.start')) render();
      else refreshTail();
    },
    refreshTail,
    get events() { return events; },
    get busy() { return loading; },
    count: () => projectTrajectory(events).length,
  };
}
