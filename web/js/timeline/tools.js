// 工具调用卡片渲染：注册表 + 三段式标题（title + subtitle + args 徽章）
// + 四态显示（running shimmer / completed / error / streaming）。

import { h, cn, button, spinner, bindTooltip } from '../ui.js';
import { renderMarkdown, reasoningHeading } from '../markdown.js';

const ICONS = {
  read: '<path d="M14 3v5h5M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/>',
  write: '<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>',
  edit: '<path d="M11 4H5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h13a2 2 0 0 0 2-2v-6"/><path d="M18.5 2.5a2.1 2.1 0 0 1 3 3L12 15l-4 1 1-4z"/>',
  skill: '<path d="M13 2 4.1 12.6a.6.6 0 0 0 .5 1h5.4l-1 8.4 8.9-10.6a.6.6 0 0 0-.5-1H12z"/>',
  terminal: '<path d="m5 8 4 4-4 4"/><path d="M12 17h7"/>',
  folder: '<path d="M4 5h5l2 2h9v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>',
  pattern: '<path d="M12 4v16M5 8l14 8M19 8 5 16"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a15 15 0 0 1 0 18 15 15 0 0 1 0-18z"/>',
  checklist: '<path d="m3 5 2 2 3-3"/><path d="M11 6h10"/><path d="m3 13 2 2 3-3"/><path d="M11 14h10"/>',
  tool: '<path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18v3h3l6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2-2z"/>',
};

function icon(kind) {
  const el = h('span', { class: 'tool-icon', 'aria-hidden': 'true' });
  el.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[kind] || ICONS.tool}</svg>`;
  return el;
}

// 每个工具的展示文案（前端生成，metadata 只提供结构化事实 —— OpenCode 分层）。
const registry = {
  read_file: {
    title: () => '读取文件',
    icon: 'read',
    subtitle: (args) => String(args.path || args.file_path || ''),
    args: (args) => [
      args.offset !== undefined && ['offset', args.offset],
      args.limit !== undefined && ['limit', args.limit],
    ].filter(Boolean),
  },
  write_file: {
    title: () => '写入文件',
    icon: 'write',
    subtitle: (args) => String(args.path || args.file_path || ''),
    args: (args) => [
      args.content !== undefined && ['bytes', String(args.content).length],
      args.append !== undefined && ['append', args.append],
    ].filter((pair) => pair && pair[1] !== undefined),
  },
  load_skill: {
    title: () => '加载技能',
    icon: 'skill',
    subtitle: (args) => String(args.name || ''),
    args: () => [],
  },
  list_files: {
    title: () => '浏览目录',
    icon: 'folder',
    subtitle: (args) => String(args.path || '.'),
    args: (args) => [
      args.depth !== undefined && ['depth', args.depth],
      args.max_entries !== undefined && ['max_entries', args.max_entries],
    ].filter(Boolean),
  },
  glob: {
    title: () => '匹配文件',
    icon: 'pattern',
    subtitle: (args) => String(args.pattern || ''),
    args: (args) => [
      args.path !== undefined && ['path', args.path],
      args.max_entries !== undefined && ['max_entries', args.max_entries],
    ].filter((pair) => pair && pair[1] !== undefined),
  },
  edit_file: {
    title: () => '编辑文件',
    icon: 'edit',
    subtitle: (args) => String(args.path || ''),
    args: (args) => [
      args.old_string !== undefined && ['old', String(args.old_string).length],
      args.new_string !== undefined && ['new', String(args.new_string).length],
      args.replace_all !== undefined && ['all', args.replace_all],
    ].filter((pair) => pair && pair[1] !== undefined),
  },
  webfetch: {
    title: () => '抓取网页',
    icon: 'globe',
    subtitle: (args) => String(args.url || ''),
    args: (args) => [
      args.max_bytes !== undefined && ['max_bytes', args.max_bytes],
    ].filter((pair) => pair && pair[1] !== undefined),
  },
  todo_write: {
    title: () => '更新任务清单',
    icon: 'checklist',
    subtitle: (args) => Array.isArray(args.items) ? `${args.items.length} 项` : '',
    args: () => [],
  },
  todo_read: {
    title: () => '读取任务清单',
    icon: 'checklist',
    subtitle: () => '',
    args: () => [],
  },
  grep: {
    title: () => '搜索内容',
    icon: 'search',
    subtitle: (args) => String(args.pattern || ''),
    args: (args) => [
      args.path !== undefined && ['path', args.path],
      args.glob !== undefined && ['glob', args.glob],
      args.ignore_case !== undefined && ['ignore_case', args.ignore_case],
    ].filter((pair) => pair && pair[1] !== undefined),
  },
};

export function registerTool(name, definition) { registry[name] = definition; }

function describe(name, args) {
  const entry = registry[name];
  if (entry) {
    return {
      title: entry.title(args),
      icon: entry.icon,
      subtitle: entry.subtitle(args) || '',
      args: entry.args(args) || [],
    };
  }
  // 通用兜底：截取主要参数。
  const pairs = Object.entries(args || {}).slice(0, 3)
    .map(([key, value]) => [key, typeof value === 'string' && value.length > 24 ? `${value.slice(0, 24)}…` : value]);
  return { title: name, icon: 'tool', subtitle: '', args: pairs };
}

function argBadges(pairs) {
  return pairs.map(([key, value]) =>
    h('span', { class: 'tool-arg mono' }, `${key}=`, h('b', {}, String(value))));
}

// 大输出钳制：max-h + 底部渐隐 + 悬浮展开（ZCode ExpandedContent 移植）。
function clampContainer(rawText, { language, cursor } = {}) {
  const pre = h('pre', { class: cn('tool-pre', language && 'mono') });
  pre.textContent = rawText;
  if (cursor) pre.append(h('span', { class: 'term-cursor', 'aria-hidden': 'true' }));
  const wrap = h('div', { class: 'clamp-wrap' }, pre);
  let expanded = false;
  const ro = new ResizeObserver(() => {
    const overflowing = pre.scrollHeight > wrap.clientHeight + 2;
    wrap.classList.toggle('is-overflowing', overflowing && !expanded);
    if (!overflowing) toggle.hidden = true;
    else toggle.hidden = false;
  });
  const toggle = button('展开', { 'data-variant': 'secondary', 'data-size': 'sm', class: 'clamp-toggle' });
  toggle.addEventListener('click', () => {
    expanded = !expanded;
    toggle.textContent = expanded ? '收起' : '展开';
    wrap.classList.toggle('is-expanded', expanded);
  });
  ro.observe(wrap);
  const container = h('div', { class: 'clamp-box' }, wrap, toggle);
  return container;
}

function ioSection(label, content, { error = false, markdown = false, cursor = false } = {}) {
  const body = markdown ? h('div', { class: 'tool-md md-body', html: renderMarkdown(content) }) : clampContainer(content, { cursor });
  return h('div', { class: 'tool-io', ...(error ? { 'data-tone': 'error' } : {}) },
    h('div', { class: 'tool-io-label' }, label),
    body,
  );
}

export function toolCard(row) {
  const info = describe(row.name, row.args);
  const running = row.status === 'running' || row.status === 'streaming';
  const failed = row.status === 'error';
  const terminal = running && row.name === 'shell';
  const card = h('article', {
    class: 'tool-card',
    'data-status': failed ? 'error' : running ? 'running' : 'completed',
  });
  const title = h('span', { class: cn('tool-title', running && 'shimmer-text') }, info.title);
  const chevron = h('span', { class: 'tool-chevron', 'aria-hidden': 'true' }, '▾');
  const trigger = h('button', {
    type: 'button',
    class: 'tool-trigger',
    'aria-expanded': 'false',
    ...(running ? { disabled: true, title: '工具正在执行…' } : {}),
  },
    icon(info.icon),
    title,
    info.subtitle ? h('span', { class: 'tool-subtitle mono', title: info.subtitle }, info.subtitle) : null,
    h('span', { class: 'tool-args' }, argBadges(info.args)),
    h('span', { class: 'tool-status' },
      running ? spinner('0.75rem')
        : h('span', { class: failed ? 'tool-flag error' : 'tool-flag ok' }, failed ? '失败' : '完成'),
      row.latencyMs ? h('span', { class: 'tool-latency mono' }, `${(row.latencyMs / 1000).toFixed(1)}s`) : null,
    ),
    running ? null : chevron,
  );
  const body = h('div', { class: 'tool-body', hidden: true });
  if (row.input && row.input !== '{}' && row.input !== 'undefined') {
    body.append(ioSection('输入', row.input, { cursor: terminal }));
  }
  if (row.result) {
    body.append(ioSection(failed ? '错误' : '输出', row.result, { error: failed, cursor: terminal }));
  }
  trigger.addEventListener('click', () => {
    const open = card.dataset.state === 'open';
    card.dataset.state = open ? 'closed' : 'open';
    body.hidden = open;
    trigger.setAttribute('aria-expanded', String(!open));
  });
  card.append(trigger, body);
  bindTooltip(card.querySelector('.tool-subtitle') || title, info.subtitle);
  return card;
}

// 思考块：OpenCode AssistantReasoningContent 移植。
// running 时标题 shimmer + 每秒计秒；完成后显示总耗时；正文 markdown 弱化排版。
export function reasoningBlock({ text, running, startedAt }) {
  const heading = reasoningHeading(text);
  const block = h('details', { class: 'reasoning-block', 'data-state': running ? 'running' : 'done' });
  const summary = h('summary', { class: 'reasoning-summary' });
  const statusLabel = h('span', { class: 'reasoning-status' });
  const duration = h('span', { class: 'reasoning-duration mono' });
  const refreshDuration = () => {
    if (running && startedAt) {
      duration.textContent = `${((Date.now() - startedAt) / 1000).toFixed(0)}s`;
    }
  };
  if (running) {
    statusLabel.append(h('span', { class: 'shimmer-text' }, '思考中'));
    summary.append(statusLabel, heading ? h('span', { class: 'reasoning-heading' }, heading) : null, duration);
    const timer = setInterval(refreshDuration, 1000);
    block.addEventListener('remove', () => clearInterval(timer));
  } else {
    statusLabel.textContent = '已思考';
    if (!Number.isNaN(Number(startedAt)) && startedAt) duration.textContent = `${(Number(startedAt) / 1000).toFixed(1)}s`;
    summary.append(statusLabel, heading ? h('span', { class: 'reasoning-heading' }, heading) : null, duration);
  }
  const body = h('div', { class: 'reasoning-body md-body', html: renderMarkdown(text) });
  block.append(summary, body);
  return block;
}
