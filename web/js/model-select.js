// 模型选择器：Command 风格面板（ZCode ModelConfigSelect + OpenCode select-dialog 移植）。
// 自动路由置顶 + tier 分组 + 模拟标记 + 当前选中勾选；Ctrl+M 打开，关闭后焦点回到输入框。

import { h, openCommand, badge, button, toast, bindTooltip } from './ui.js';
import { tierNames } from './store.js';

function contextHint(model) {
  const windowTokens = Number(model.context_window || 0);
  const parts = [String(model.provider || '')];
  if (windowTokens) parts.push(`${Math.round(windowTokens / 1000)}k 上下文`);
  return parts.filter(Boolean).join(' · ');
}

export function modelItems(state) {
  const items = [{
    value: '__auto__',
    label: '自动路由',
    hint: '按任务复杂度与模型能力自动选择层级',
    group: '路由',
  }];
  const tiers = state.tiers.length ? state.tiers : Object.keys(tierNames).map((id) => ({ id, name: tierNames[id] }));
  for (const tier of tiers) {
    const tierId = String(tier.id || tier.name || '');
    if (!tierId) continue;
    const label = tier.label || tier.name || tierNames[tierId] || tierId;
    const models = state.models.filter((model) => String(model.tier || '').toLowerCase() === tierId);
    if (!models.length) {
      items.push({
        value: `tier:${tierId}`,
        label: `${label} · 层级`,
        hint: '该层级暂无已配置模型',
        group: '层级',
        disabled: true,
      });
      continue;
    }
    for (const model of models) {
      const simulated = state.simulation || String(model.provider || '').toLowerCase() === 'demo';
      items.push({
        value: `model:${model.id}`,
        label: model.id,
        hint: contextHint(model),
        group: `${label}（${tierId}）`,
        badge: simulated ? badge('模拟', 'warning') : badge('真实', 'success'),
      });
    }
  }
  return items;
}

export function requestSummary(state) {
  const { model, tier } = state.request;
  if (model) return model;
  if (tier) return `${tierNames[tier] || tier} · 自动选择`;
  return state.request.automatic ? '自动路由' : '按复杂度路由';
}

export function isCurrentItem(state, value) {
  if (value === '__auto__') return !state.request.model && !state.request.tier;
  if (value.startsWith('model:')) return state.request.model === value.slice(6);
  if (value.startsWith('tier:')) return !state.request.model && state.request.tier === value.slice(5);
  return false;
}

export function openModelSelect(anchor, state, { onPick, restoreFocus }) {
  openCommand(anchor, {
    items: modelItems(state),
    placeholder: '搜索模型或层级…',
    footer: [
      h('span', {}, 'Enter 选择'),
      h('span', {}, '↑↓ 导航'),
      h('span', {}, 'Ctrl+M 打开'),
    ],
    onPick: (value) => {
      if (value === '__auto__') {
        state.request.tier = '';
        state.request.model = '';
      } else if (value.startsWith('model:')) {
        state.request.model = value.slice(6);
        state.request.tier = '';
      } else if (value.startsWith('tier:')) {
        state.request.tier = value.slice(5);
        state.request.model = '';
      }
      toast(`模型选择：${requestSummary(state)}`);
      onPick?.();
    },
    onClose: () => restoreFocus?.(),
  });
}

// Composer 左下角的模型选择按钮。
export function modelSelectButton(state, { onPick }) {
  const label = h('span', { class: 'model-chip-label' }, requestSummary(state));
  const btn = button(
    h('span', { class: 'model-chip-dot', 'aria-hidden': 'true' }),
    label,
    h('span', { class: 'model-chip-chevron', 'aria-hidden': 'true' }, '▾'),
    {
      'data-variant': 'ghost',
      'data-size': 'sm',
      class: 'model-chip',
      'aria-haspopup': 'listbox',
      onclick: (event) => openModelSelect(event.currentTarget, state, {
        onPick: () => { label.textContent = requestSummary(state); onPick?.(); },
        restoreFocus: () => btn.focus(),
      }),
    });
  bindTooltip(btn, () => `模型选择：${label.textContent}（Ctrl+M）`);
  return btn;
}
