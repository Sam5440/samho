// 模型面板：任务类型（路由层级）→ 模型分配。
// 每个任务类型对应一类任务；服务端 RoleRouter 在路由决策后按类型改绑模型。
// 空分配 = 跟随自动路由；默认模型由服务端种子（glm-5.3-flash 若已配置）。

import { h, badge, button, toast } from '../ui.js';
import { api } from '../api.js';

const typeDescriptions = {
  nano: ['轻量问答', '简单查询与短文本处理'],
  fast: ['快速任务', '翻译、摘要等低复杂度工作'],
  balanced: ['常规对话与规划', '日常任务分析、对话与执行规划'],
  reasoning: ['深度推理', '调试、复杂问题拆解与推理链'],
  frontier: ['复杂架构', '架构设计、证明与重写级任务'],
  specialist: ['编码专精', '以代码为中心的专项任务'],
};

function rangeText(tier) {
  const min = Number(tier.min_complexity);
  const max = Number(tier.max_complexity);
  if (min === 255 && max === 255) return '手动层级';
  if (min >= max) return `复杂度 ${min}`;
  return `复杂度 ${min}–${max}`;
}

export function createModelsPanel({ container }) {
  let data = null;

  function modelOptions(tierId, selected) {
    const select = h('select', { class: 'models-select', 'aria-label': '该任务类型使用的模型' });
    select.append(h('option', { value: '' }, '跟随自动路由'));
    const byTier = new Map();
    for (const model of data.models) {
      const tier = String(model.tier || 'other');
      if (!byTier.has(tier)) byTier.set(tier, []);
      byTier.get(tier).push(model);
    }
    for (const [tier, models] of byTier) {
      const group = h('optgroup', { label: tier });
      for (const model of models) {
        const option = h('option', { value: model.id }, model.id);
        if (model.id === selected) option.selected = true;
        group.append(option);
      }
      select.append(group);
    }
    select.addEventListener('change', () => save(tierId, select.value));
    return select;
  }

  function render() {
    if (!data) return;
    const tiers = [...data.tiers].sort((a, b) => Number(a.rank) - Number(b.rank));
    const rows = tiers.map((tier) => {
      const [title, description] = typeDescriptions[tier.id] || [tier.id, ''];
      const current = String(data.roles[tier.id] ?? '');
      return h('div', { class: 'models-row' },
        h('div', { class: 'models-row-copy min-w-0' },
          h('span', { class: 'models-row-title' },
            title,
            current
              ? badge(current === data.default_model ? '默认' : '已指定', current === data.default_model ? 'secondary' : 'success')
              : badge('自动', 'secondary')),
          h('span', { class: 'text-ui-xs text-subtle' },
            `${description} · ${rangeText(tier)}`)),
        modelOptions(tier.id, current));
    });
    container.replaceChildren(
      h('div', { class: 'models-head' },
        h('p', { class: 'text-ui-sm text-muted' },
          '为不同类型的任务分配模型；发送时未显式选择模型则按任务类型改绑。',
          data.default_model
            ? h('span', {}, `默认：${data.default_model}。`)
            : null),
        h('div', { class: 'flex gap-2' },
          button('全部恢复默认', {
            'data-variant': 'ghost', 'data-size': 'sm',
            disabled: !data.default_model,
            onclick: () => applyAll(data.default_model || ''),
          })),
      ),
      h('div', { class: 'models-list' }, ...rows));
  }

  async function applyAll(model) {
    const roles = Object.fromEntries(data.tiers.map((tier) => [tier.id, model]));
    await saveRoles(roles, '已恢复默认分配');
  }

  async function saveRoles(roles, okMessage) {
    try {
      const saved = await api('/api/model-roles', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ roles }),
      });
      data.roles = saved.roles || roles;
      toast(okMessage);
      render();
    } catch (error) {
      toast(`保存失败：${error.message}`, 'error');
    }
  }

  function save(tierId, model) {
    if (!data) return;
    saveRoles({ ...data.roles, [tierId]: model }, '模型分配已保存');
  }

  async function refresh() {
    try {
      data = await api('/api/model-roles');
      render();
    } catch (error) {
      container.replaceChildren(
        h('p', { class: 'text-ui-sm text-muted' }, `加载失败：${error.message}`));
    }
  }

  return {
    refresh,
    show() { return refresh(); },
    hide() {},
  };
}
