// Motion 层（Motion registry 的 vanilla 源码入仓子集）：
// reduce-motion 门控 + RollingNumber（数字滚柱）+ HaloProgress（晕环进度）。
// 约束：只动 transform / opacity；元素重建（首挂载）或 reduce-motion 时直接落位。

const reducedQuery = window.matchMedia('(prefers-reduced-motion: reduce)');

export function prefersReduced() {
  return reducedQuery.matches;
}

/* ---------- RollingNumber ----------
   每一位数字一根 0-9 纵列，translateY 切换（transform-only）。
   el 持久存在且结构不变 → 平滑滚动；否则直接落位（无动画）。
   用法：const span = h('span', { class: 'roll-num' }); rollingNumber(span, 12345); */
export function rollingNumber(el, value, { format } = {}) {
  const text = typeof format === 'function'
    ? format(value)
    : Number(value || 0).toLocaleString('zh-CN');
  if (prefersReduced()) {
    el.textContent = text;
    el.removeAttribute('aria-label');
    el.__rollText = null;
    return el;
  }
  const pattern = Array.from(text).map((ch) => (/\d/.test(ch) ? 'd' : ch)).join('');
  if (el.__rollText === text) return el;
  if (el.__rollPattern !== pattern || !el.__rollSlots) {
    el.replaceChildren();
    el.__rollSlots = [];
    el.setAttribute('aria-label', text);
    for (const ch of text) {
      if (/\d/.test(ch)) {
        const col = h('span', { class: 'roll-col', 'aria-hidden': 'true' });
        for (let d = 0; d <= 9; d++) col.append(h('span', {}, String(d)));
        col.style.transform = `translateY(-${Number(ch)}em)`;
        el.append(col);
        el.__rollSlots.push(col);
      } else {
        el.append(h('span', { 'aria-hidden': 'true' }, ch));
        el.__rollSlots.push(null);
      }
    }
  } else {
    // 同构：仅数字变化，逐列滚动。
    el.setAttribute('aria-label', text);
    let index = 0;
    for (const ch of text) {
      const col = el.__rollSlots[index++];
      if (col) col.style.transform = `translateY(-${Number(ch)}em)`;
    }
  }
  el.__rollText = text;
  el.__rollPattern = pattern;
  return el;
}

/* ---------- HaloProgress ----------
   结构：track + fill（scaleX 确定态 / translateX 不定态）+ 前沿光点（opacity 呼吸）。
   setHalo(el, value, max)：max>0 为确定态百分比，否则不定态。 */
export function haloProgress(el, { label } = {}) {
  if (!el.__haloInit) {
    el.__haloInit = true;
    el.classList.add('halo-progress');
    el.append(
      h('span', { class: 'halo-track', 'aria-hidden': 'true' },
        h('span', { class: 'halo-fill' }),
        h('span', { class: 'halo-spark' })),
    );
  }
  if (label) el.setAttribute('aria-label', label);
  return el;
}

export function setHalo(el, value, max = 0) {
  haloProgress(el);
  if (max > 0) {
    const ratio = Math.max(0, Math.min(1, Number(value || 0) / max));
    el.dataset.mode = 'determinate';
    el.style.setProperty('--halo-v', ratio.toFixed(4));
    el.setAttribute('aria-valuemin', '0');
    el.setAttribute('aria-valuemax', String(max));
    el.setAttribute('aria-valuenow', String(Math.round(Number(value || 0))));
  } else {
    el.dataset.mode = 'indeterminate';
    el.style.removeProperty('--halo-v');
    el.removeAttribute('aria-valuemin');
    el.removeAttribute('aria-valuemax');
    el.removeAttribute('aria-valuenow');
  }
  return el;
}

/* ---------- 局部 h()（避免循环依赖 ui.js） ---------- */
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'class') el.className = value;
    else if (value === true) el.setAttribute(key, '');
    else el.setAttribute(key, String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child === undefined || child === null || child === false) continue;
    el.append(child.nodeType ? child : document.createTextNode(String(child)));
  }
  return el;
}
