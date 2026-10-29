// 右栏停靠：两组 tab（设备：文件/Git/终端/浏览器；检查：轨迹/对象/MCP/技能）。
// 面板互斥显示；组标签只是「跳转」入口，记住各组最近使用的 tab。

import { h, cn } from './ui.js';

export function createDock({ devicePanels, inspectPanels, defaultDevice = 'files' }) {
  // name -> { activate(): Promise|void }
  const panels = new Map([...Object.entries(devicePanels), ...Object.entries(inspectPanels)]);
  const groupOf = new Map([
    ...Object.keys(devicePanels).map((name) => [name, 'device']),
    ...Object.keys(inspectPanels).map((name) => [name, 'inspect']),
  ]);
  const labels = {
    files: '文件', git: 'Git', term: '终端', browser: '浏览器',
    trajectory: '轨迹', agents: '子代理', tasks: '任务', objects: '对象', mcp: 'MCP', skills: '技能', models: '模型',
  };
  let active = null;
  let lastByGroup = { device: defaultDevice, inspect: 'trajectory' };

  const groupRow = (group) => h('div', { class: 'dock-group', role: 'group', 'aria-label': group === 'device' ? '设备面板' : '检查面板' },
    h('span', { class: 'dock-group-label', 'aria-hidden': 'true' }, group === 'device' ? '设备' : '检查'),
    h('div', { class: 'dock-tabs', role: 'tablist', 'aria-label': group === 'device' ? '设备工具' : '运行时检查' },
      Object.keys(group === 'device' ? devicePanels : inspectPanels).map((name) =>
        h('button', {
          type: 'button', role: 'tab', class: 'dock-tab', 'data-panel': name,
          'aria-selected': 'false', title: labels[name] || name,
          onclick: () => activate(name),
        }, labels[name] || name)),
    ),
  );
  const deviceRow = groupRow('device');
  const inspectRow = groupRow('inspect');
  const status = h('span', { class: 'text-ui-xs text-subtle dock-status' });

  function syncTabs() {
    for (const row of [deviceRow, inspectRow]) {
      for (const tab of row.querySelectorAll('[data-panel]')) {
        const name = tab.dataset.panel;
        tab.setAttribute('aria-selected', String(name === active));
      }
    }
  }

  function syncPanels() {
    for (const name of panels.keys()) {
      const section = document.getElementById(`panel-${name}`);
      if (section) section.hidden = name !== active;
    }
  }

  async function activate(name) {
    if (!panels.has(name)) { syncTabs(); return; }
    if (name === active) {
      await panels.get(name).show?.();
      syncTabs();
      return;
    }
    const previous = active;
    active = name;
    lastByGroup[groupOf.get(name)] = name;
    syncTabs();
    for (const [panelName, panel] of panels) {
      if (panelName === name) continue;
      panel.hide?.();
    }
    try {
      await panels.get(name).show?.();
    } catch (error) {
      active = previous;
      syncTabs();
      throw error;
    }
    syncPanels();
  }

  const container = h('div', { class: 'dock-head' }, deviceRow, inspectRow, status);

  return {
    el: container,
    statusEl: status,
    async activate(name) { await activate(name); },
    get active() { return active; },
    // 重新执行当前面板的 show（工作区切换后按新前缀刷新数据）。
    async refreshActive() {
      if (active) await panels.get(active).show?.();
    },
    // 切换到某组最近使用的面板（工作区切换后设备面板需要刷新时用）。
    async reveal(group) {
      const target = lastByGroup[group] || (group === 'device' ? defaultDevice : 'trajectory');
      if (active === target) { await refreshActive(); return; }
      await activate(target);
    },
  };
}
