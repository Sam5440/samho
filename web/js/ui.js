// shadcn/ui 风格组件的 vanilla 行为层：h() 元素构建 + 受控浮层 + 命令面板。
// 全部样式由 ui/components.css 的 data-* 属性驱动；动态定位走 CSSOM（CSP 允许）。

export function cn(...parts) {
  return parts.filter(Boolean).join(' ');
}

export function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'class') el.className = cn(value);
    else if (key === 'text') el.textContent = value;
    else if (key === 'html') el.innerHTML = value; // 调用方保证已转义
    else if (key === 'style') Object.assign(el.style, value);
    else if (key === 'dataset') Object.assign(el.dataset, value);
    else if (key.startsWith('on') && typeof value === 'function') el.addEventListener(key.slice(2), value);
    else if (value === true) el.setAttribute(key, '');
    else el.setAttribute(key, String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child === undefined || child === null || child === false) continue;
    el.append(child.nodeType ? child : document.createTextNode(String(child)));
  }
  return el;
}

export const spinner = (size) => {
  const svg = h('span', { 'data-component': 'spinner', 'aria-hidden': 'true' });
  svg.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M22 12a10 10 0 0 0-10-10"/></svg>';
  if (size) { svg.style.width = size; svg.style.height = size; }
  return svg;
};

export const kbd = (text) => h('span', { 'data-component': 'kbd' }, text);

export const button = (text, props = {}, ...children) => {
  // 宽容签名：元素与 props 对象可在任意参数位，统一归类。
  const options = {};
  const rest = [];
  for (const arg of [text, props, ...children]) {
    if (!arg) continue;
    if (arg.nodeType) { rest.push(arg); continue; }
    if (typeof arg === 'object') { Object.assign(options, arg); continue; }
    rest.push(arg);
  }
  return h('button', { type: 'button', 'data-component': 'button', ...options }, ...rest);
};

export const badge = (text, variant = 'secondary', mono = false) =>
  h('span', { 'data-component': 'badge', 'data-variant': variant, class: mono ? 'mono' : '' }, text);

/* ---------- 浮层定位 ---------- */
export function positionFloating(el, anchor, { align = 'start', margin = 6, matchWidth = false } = {}) {
  const a = anchor.getBoundingClientRect();
  el.style.visibility = 'hidden';
  document.body.append(el);
  const r = el.getBoundingClientRect();
  let left = align === 'end' ? a.right - r.width : a.left;
  left = Math.max(margin, Math.min(left, window.innerWidth - r.width - margin));
  let top = a.bottom + margin;
  if (top + r.height > window.innerHeight - margin) {
    const above = a.top - margin - r.height;
    top = above >= margin ? above : Math.max(margin, window.innerHeight - r.height - margin);
  }
  el.style.left = `${Math.round(left)}px`;
  el.style.top = `${Math.round(top)}px`;
  if (matchWidth) el.style.minWidth = `${Math.round(a.width)}px`;
  el.style.visibility = '';
  return { left, top };
}

function closeOnOutside(container, anchor, done) {
  const onDown = (event) => {
    if (!container.contains(event.target) && !(anchor && anchor.contains(event.target))) finish();
  };
  const onKey = (event) => { if (event.key === 'Escape') { event.stopPropagation(); finish(); } };
  function finish() {
    document.removeEventListener('pointerdown', onDown, true);
    document.removeEventListener('keydown', onKey, true);
    done();
  }
  document.addEventListener('pointerdown', onDown, true);
  document.addEventListener('keydown', onKey, true);
  return finish;
}

/* ---------- DropdownMenu ---------- */
export function openMenu(anchor, items, { align = 'start', onSelect } = {}) {
  const content = h('div', { role: 'menu', 'data-component': 'menu-content' });
  const entries = [];
  let index = -1;
  for (const item of items) {
    if (item === '-') { content.append(h('div', { 'data-slot': 'menu-separator' })); continue; }
    if (item.label && item.heading) { content.append(h('div', { 'data-slot': 'menu-label' }, item.label)); continue; }
    const row = h('button', {
      type: 'button',
      role: 'menuitem',
      'data-slot': 'menu-item',
      'data-variant': item.variant,
      disabled: item.disabled ? true : null,
    },
      item.icon || null,
      h('span', { class: 'min-w-0', style: { flex: '1 1 auto', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' } }, item.label),
      item.shortcut ? h('span', { 'data-slot': 'menu-shortcut' }, item.shortcut) : null,
    );
    row.addEventListener('click', () => { close(); (item.onSelect || onSelect)?.(item.value, item); });
    entries.push(row);
    content.append(row);
  }
  let closed = false;
  const close = closeOnOutside(content, anchor, () => { closed = true; content.remove(); });
  positionFloating(content, anchor, { align });
  const move = (delta) => {
    if (!entries.length) return;
    index = (index + delta + entries.length) % entries.length;
    entries.forEach((row, i) => row.dataset.highlighted = String(i === index));
    entries[index].focus();
  };
  content.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown') { event.preventDefault(); move(1); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); move(-1); }
    else if (event.key === 'Enter' && index >= 0) { event.preventDefault(); entries[index].click(); }
  });
  (entries[0] || content).focus?.();
  return { close: () => { if (!closed) { content.remove(); closed = true; } } };
}

/* ---------- Popover ---------- */
export function openPopover(anchor, contentEl, opts = {}) {
  let closed = false;
  const close = closeOnOutside(contentEl, anchor, () => { closed = true; contentEl.remove(); opts.onClose?.(); });
  positionFloating(contentEl, anchor, opts);
  return { close: () => { if (!closed) { contentEl.remove(); closed = true; opts.onClose?.(); } } };
}

/* ---------- Tooltip ---------- */
let tooltipEl = null;
let tooltipTimer = 0;
export function bindTooltip(el, getText) {
  el.addEventListener('pointerenter', (event) => {
    if (event.pointerType !== 'mouse') return;
    clearTimeout(tooltipTimer);
    tooltipTimer = setTimeout(() => {
      const text = typeof getText === 'function' ? getText() : getText;
      if (!text) return;
      tooltipEl?.remove();
      tooltipEl = h('div', { role: 'tooltip', 'data-component': 'tooltip' }, text);
      document.body.append(tooltipEl);
      const r = el.getBoundingClientRect();
      const t = tooltipEl.getBoundingClientRect();
      let left = r.left + r.width / 2 - t.width / 2;
      left = Math.max(4, Math.min(left, window.innerWidth - t.width - 4));
      let top = r.top - t.height - 6;
      if (top < 4) top = r.bottom + 6;
      tooltipEl.style.left = `${Math.round(left)}px`;
      tooltipEl.style.top = `${Math.round(top)}px`;
    }, 350);
  });
  el.addEventListener('pointerleave', () => { clearTimeout(tooltipTimer); tooltipEl?.remove(); tooltipEl = null; });
  el.addEventListener('pointerdown', () => { clearTimeout(tooltipTimer); tooltipEl?.remove(); tooltipEl = null; });
}

/* ---------- Dialog ---------- */
export function openDialog({ title, description, body, footer, size, onClose }) {
  const overlay = h('div', { 'data-component': 'dialog-overlay' });
  const dialog = h('div', {
    role: 'dialog',
    'aria-modal': 'true',
    'data-component': 'dialog',
    ...(size ? { 'data-size': size } : {}),
  },
    h('div', { 'data-slot': 'dialog-header' },
      h('div', {},
        h('div', { 'data-slot': 'dialog-title' }, title),
        description ? h('div', { 'data-slot': 'dialog-description' }, description) : null,
      ),
      button('✕', { 'data-variant': 'ghost', 'data-size': 'icon-sm', 'aria-label': '关闭', onclick: () => close() }),
    ),
    body ? h('div', { 'data-slot': 'dialog-body' }, body) : null,
    footer ? h('div', { 'data-slot': 'dialog-footer' }, footer) : null,
  );
  const close = () => { overlay.remove(); dialog.remove(); document.removeEventListener('keydown', onKey, true); onClose?.(); };
  const onKey = (event) => {
    if (event.key === 'Escape') { event.stopPropagation(); close(); }
    else if (event.key === 'Tab') {
      const focusables = dialog.querySelectorAll('button, input, textarea, select, [tabindex]:not([tabindex="-1"])');
      if (!focusables.length) return;
      const list = Array.from(focusables).filter((el) => !el.disabled);
      const first = list[0], last = list[list.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      else if (!dialog.contains(document.activeElement)) { event.preventDefault(); first.focus(); }
    }
  };
  overlay.addEventListener('pointerdown', close);
  document.addEventListener('keydown', onKey, true);
  document.body.append(overlay, dialog);
  const focusable = dialog.querySelector('input, textarea, button:not([aria-label="关闭"])');
  focusable?.focus();
  return { close, element: dialog };
}

/* ---------- Toast ---------- */
let toaster = null;
export function toast(message, variant = 'default') {
  if (!toaster) { toaster = h('div', { 'data-component': 'toaster', 'aria-live': 'polite' }); document.body.append(toaster); }
  const item = h('div', { role: 'status', 'data-component': 'toast', ...(variant !== 'default' ? { 'data-variant': variant } : {}) },
    variant === 'error' ? h('span', { 'aria-hidden': 'true', style: { color: 'var(--destructive)' } }, '⚠') : null,
    h('span', { 'data-slot': 'toast-message' }, message),
  );
  const dismiss = () => { item.remove(); if (!toaster.childElementCount) { toaster.remove(); toaster = null; } };
  item.addEventListener('click', dismiss);
  toaster.append(item);
  setTimeout(dismiss, variant === 'error' ? 8000 : 4000);
}

/* ---------- Command（模型选择器等搜索面板） ---------- */
// items: [{ value, label, hint, badge, group, disabled, meta }]
export function openCommand(anchor, { items, placeholder = '搜索…', footer, onPick, onClose, emptyText = '没有匹配项' } = {}) {
  const input = h('input', { 'data-slot': 'command-input', placeholder, spellcheck: 'false' });
  const list = h('div', { 'data-slot': 'command-list', role: 'listbox' });
  const content = h('div', { 'data-component': 'command' },
    h('div', { 'data-slot': 'command-input-wrap' },
      h('span', { 'aria-hidden': 'true', class: 'text-subtle' }, '⌕'), input,
      kbd('Esc'),
    ),
    list,
    footer ? h('div', { 'data-slot': 'command-footer' }, footer) : null,
  );
  let selected = 0;
  let visible = [];

  function match(item, query) {
    if (!query) return true;
    return `${item.label} ${item.hint || ''} ${item.group || ''}`.toLowerCase().includes(query);
  }

  function render() {
    const query = input.value.trim().toLowerCase();
    visible = items.filter((item) => match(item, query));
    list.replaceChildren();
    if (!visible.length) { list.append(h('div', { 'data-slot': 'command-empty' }, emptyText)); return; }
    selected = Math.min(selected, visible.length - 1);
    let lastGroup = null;
    visible.forEach((item, i) => {
      if (item.group && item.group !== lastGroup && (!query || true)) {
        list.append(h('div', { 'data-slot': 'command-group-label' }, item.group));
        lastGroup = item.group;
      }
      const row = h('div', {
        role: 'option',
        'aria-selected': String(i === selected),
        'data-slot': 'command-item',
        'data-value': item.value,
        style: item.disabled ? { opacity: '0.5', pointerEvents: 'none' } : undefined,
      },
        h('span', { class: 'min-w-0' },
          h('span', { style: { display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' } }, item.label),
          item.hint ? h('span', { class: 'text-ui-xs text-subtle', style: { display: 'block' } }, item.hint) : null,
        ),
        item.badge || null,
        item.meta || null,
        i === selected ? h('span', { 'data-slot': 'command-check', 'aria-hidden': 'true' }, '✓') : null,
      );
      row.addEventListener('click', () => pick(item));
      row.addEventListener('pointermove', () => { if (selected !== i) { selected = i; paint(); } });
      list.append(row);
    });
  }

  function paint() {
    list.querySelectorAll('[data-slot="command-item"]').forEach((row, i) => {
      row.setAttribute('aria-selected', String(i === selected));
      row.querySelector('[data-slot="command-check"]')?.remove();
      if (i === selected) row.append(h('span', { 'data-slot': 'command-check', 'aria-hidden': 'true' }, '✓'));
    });
    visible[selected] && list.children[visible.slice(0, selected).length + countGroupsBefore(selected)]?.scrollIntoView({ block: 'nearest' });
  }

  function countGroupsBefore(index) {
    let groups = 0, lastGroup = null;
    for (let i = 0; i < index; i++) {
      if (visible[i].group && visible[i].group !== lastGroup) { groups++; lastGroup = visible[i].group; }
    }
    return groups;
  }

  function pick(item) {
    if (item.disabled) return;
    done();
    onPick?.(item.value, item);
  }

  function done() {
    document.removeEventListener('keydown', onKey, true);
    content.remove();
    onClose?.();
  }

  function onKey(event) {
    if (event.key === 'Escape') { event.stopPropagation(); done(); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); selected = Math.min(selected + 1, visible.length - 1); paint(); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); selected = Math.max(selected - 1, 0); paint(); }
    else if (event.key === 'Enter') { event.preventDefault(); const item = visible[selected]; if (item) pick(item); }
  }

  input.addEventListener('input', () => { selected = 0; render(); });
  document.addEventListener('keydown', onKey, true);
  positionFloating(content, anchor, { align: 'start', matchWidth: false });
  content.style.width = 'min(26rem, 90vw)';
  content.style.maxHeight = 'min(28rem, 80vh)';
  render();
  input.focus();
  return { close: done };
}

/* ---------- 两击确认按钮（归档/删除保护） ---------- */
export function armConfirm(btn, { confirmText = '确认？', timeout = 2500, onConfirm }) {
  let armed = false;
  let timer = 0;
  const original = btn.textContent;
  btn.addEventListener('click', () => {
    if (!armed) {
      armed = true;
      btn.textContent = confirmText;
      btn.dataset.variant = 'destructive';
      timer = setTimeout(() => { armed = false; btn.textContent = original; btn.dataset.variant = 'ghost'; }, timeout);
      return;
    }
    clearTimeout(timer);
    armed = false;
    btn.textContent = original;
    btn.dataset.variant = 'ghost';
    onConfirm();
  });
  btn.addEventListener('pointerleave', () => {
    // 鼠标离开不取消（ZCode 注释：避免误触保护失效），仅超时复位。
  });
}

/* ---------- 相对时间 ---------- */
export function relativeTime(ms) {
  if (!ms) return '';
  const diff = Date.now() - Number(ms);
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  return new Date(Number(ms)).toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' });
}

export function clockTime(ms) {
  return new Date(Number(ms)).toLocaleTimeString('zh-CN', { hour12: false });
}
