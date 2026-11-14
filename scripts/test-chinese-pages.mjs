import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');
const hasChinese = (text) => /[\u3400-\u4dbf\u4e00-\u9fff]/u.test(text);

test('工作台、文档页和确认计划页声明中文语言并提供中文入口', () => {
  // 工作台是 React 应用：HTML 外壳在 web/app/index.html，可见文案在组件源码中。
  const shell = read('web/app/index.html');
  const index = read('web/app/src/components/topbar.tsx');
  const docs = read('web/docs.html');
  const plan = read('web/plan.html');
  for (const html of [shell, docs, plan]) {
    assert.match(html, /<html\s+lang=["']zh-CN["']/i);
    assert.match(html, /<meta\s+charset=["']utf-8["']/i);
    assert.ok(hasChinese(html), '页面应包含中文可见文案');
  }
  assert.match(index, /中文文档/);
  assert.match(index, /确认计划/);
  assert.match(docs, /下载当前中文文档/);
  assert.match(docs, /打开确认计划页/);
  assert.match(plan, /生成完整确认计划/);
  assert.match(plan, /回传对话/);
  assert.match(plan, /不会执行任务/);
  // 这些是上一版的英文展示标题；技术字段可以保留英文，但页面标题必须中文。
  for (const oldLabel of ['YOUR WORKSPACE', 'AGENT WORKBENCH', 'RUNTIME INSPECTOR', 'EXECUTION TRACE', 'OBJECT REGISTRY', 'SKILL LIBRARY', 'EVERYTHING IS AN OBJECT']) {
    assert.doesNotMatch(index, new RegExp(oldLabel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  }
});

test('中文资料包包含 12 份文档且每份具有中文元数据', () => {
  const source = read('web/documents-data.js');
  const sandbox = { window: {} };
  vm.runInNewContext(source, sandbox, { filename: 'web/documents-data.js' });
  const docs = sandbox.window.SamhoDocuments;
  assert.ok(Array.isArray(docs));
  assert.equal(docs.length, 12);
  const ids = new Set();
  for (const doc of docs) {
    assert.ok(doc && typeof doc === 'object');
    assert.match(String(doc.id), /^[a-z0-9-]+$/);
    assert.ok(!ids.has(doc.id), `文档 id 重复：${doc.id}`);
    ids.add(doc.id);
    assert.ok(String(doc.title).trim() && hasChinese(doc.title), `标题缺少中文：${doc.id}`);
    assert.ok(String(doc.category).trim() && hasChinese(doc.category), `分类缺少中文：${doc.id}`);
    assert.ok(String(doc.path).trim());
    assert.ok(String(doc.body).trim() && hasChinese(doc.body), `正文缺少中文：${doc.id}`);
    assert.equal(doc.body.replace(/\r\n/g, '\n'), read(doc.path).replace(/^\uFEFF/, '').replace(/\r\n/g, '\n'), `网页资料包需要与原文同步：${doc.path}`);
  }
});

test('文档 Markdown 渲染使用安全 DOM API', () => {
  const source = read('web/docs.js');
  for (const unsafe of ['innerHTML', 'outerHTML', 'insertAdjacentHTML', 'document.write', 'eval(', 'new Function', 'javascript:']) {
    assert.equal(source.includes(unsafe), false, `docs.js 不应使用 ${unsafe}`);
  }
  assert.match(source, /createElement/);
  assert.match(source, /createTextNode/);
  assert.match(source, /textContent/);
  assert.match(source, /noopener noreferrer/);
});

test('确认计划页是离线编排器，不调用任务 API 或外部网络', () => {
  const html = read('web/plan.html');
  const source = read('web/plan.js');
  // 计划模块的注释会解释“不会调用 /api/run”；只在可执行代码上检查调用。
  const executable = source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|\s)\/\/.*$/gm, '$1');
  for (const networkToken of ['fetch(', 'XMLHttpRequest', 'WebSocket', 'EventSource', '/api/run', '/api/tasks']) {
    assert.equal(executable.includes(networkToken), false, `计划页不应包含网络调用：${networkToken}`);
    assert.equal(html.includes(networkToken), false, `计划页不应包含任务接口：${networkToken}`);
  }
  assert.match(source, /global\.SamhoPlan/);
  assert.match(source, /generatePlanText/);
  assert.match(source, /validatePlan/);
  assert.match(source, /computeDependencies/);
  assert.match(source, /new Blob/);
  assert.match(source, /text\/plain;charset=utf-8/);
  for (const unsafe of ['innerHTML', 'outerHTML', 'insertAdjacentHTML', 'eval(', 'new Function']) {
    assert.equal(source.includes(unsafe), false, `plan.js 不应使用 ${unsafe}`);
  }
  assert.doesNotMatch(html, /<(?:script|link)\b[^>]*(?:src|href)=["']https?:/i, '离线页面不能依赖外部脚本或样式');
  assert.doesNotMatch(html, /\son\w+=["']/i, '计划页事件由本地脚本注册，不应插入内联事件');
  assert.match(html, /href=["']docs\.html["']/);
  assert.match(html, /href=["']index\.html["']/);
});

function planApi() {
  const sandbox = { console, Date, globalThis: {} };
  vm.runInNewContext(read('web/plan.js'), sandbox, { filename: 'web/plan.js' });
  assert.ok(sandbox.globalThis.SamhoPlan, 'plan.js 应暴露纯函数 API');
  return sandbox.globalThis.SamhoPlan;
}

function allSelected(api) {
  return Object.fromEntries(api.groups.flatMap((group) => group.items.map((item) => [item.id, true])));
}

test('计划纯函数 API 输出确定性的完整中文文本', () => {
  const api = planApi();
  const data = {
    projectName: '测试项目',
    target: '验证中文计划导出',
    priority: '可靠性',
    boundary: '仅在当前仓库本地验证',
    mode: '本地 Web GUI 与 TUI 共用 Rust 服务',
    models: '仅使用模拟模型',
    budget: '不调用付费 API',
    platforms: ['Windows'],
    scenarios: ['计划生成'],
    notes: '保留中文与 <script> 文本',
    selected: allSelected(api),
  };
  const options = { date: '2026-10-07' };
  const first = api.generatePlanText(data, options);
  const second = api.generatePlanText(data, options);
  assert.equal(first, second);
  assert.match(first, /Samho Rust Agent 重写/);
  assert.match(first, /生成日期：2026-10-07/);
  assert.match(first, /测试项目/);
  assert.match(first, /验证中文计划导出/);
  assert.match(first, /保留中文与 <script> 文本/);
  assert.match(first, /未勾选范围/);
  assert.match(first, /当前 MCP 仅为已验证方法子集/);
  assert.match(first, /性能目标必须通过同机/);
  assert.equal(api.filename(data, options), 'samho-确认执行计划-2026-10-07.txt');
  assert.equal(api.validatePlan(data).valid, true);
});

test('空范围、缺目标和未知范围不能通过计划校验', () => {
  const api = planApi();
  const empty = api.validatePlan({});
  assert.equal(empty.valid, false);
  assert.ok(empty.errors.some((error) => error.includes('项目目标')));
  assert.ok(empty.errors.some((error) => error.includes('至少勾选')));
  assert.ok(empty.errors.some((error) => error.includes('最高优先级')));
  const unknown = api.validatePlan({ target: '目标', priority: '优先', selected: { 'not-a-real-item': true } });
  assert.equal(unknown.valid, false);
  const mixed = api.validatePlan({ target: '目标', priority: '优先', selected: { 'model-routing': true, 'not-a-real-item': true } });
  assert.equal(mixed.valid, false, '合法项不能掩盖未知项');
  const text = api.generatePlanText({ target: '目标', priority: '优先', selected: { 'not-a-real-item': true } }, { date: '2026-10-07' });
  assert.doesNotMatch(text, /not-a-real-item/);
});

test('计划依赖检查报告缺口且不擅自补选', () => {
  const api = planApi();
  const warnings = api.computeDependencies({ selected: { 'web-gui': true } });
  assert.ok(warnings.some((warning) => warning.missing.includes('shared-runtime')));
  const mcpWarnings = api.computeDependencies({ selected: { 'mcp-optional': true } });
  assert.ok(mcpWarnings.some((warning) => warning.missing.includes('mcp-support')));
  const data = { target: '目标', priority: '优先', selected: { 'web-gui': true } };
  const result = api.validatePlan(data);
  assert.equal(result.valid, true);
  assert.ok(result.warnings.length > 0);
  assert.equal(data.selected['shared-runtime'], undefined);
});

test('47 项确认范围具有唯一身份与明确状态，未实现能力不被标为完成', () => {
  const api = planApi();
  const items = api.groups.flatMap((group) => group.items);
  assert.equal(items.length, 47);
  assert.equal(new Set(items.map((item) => item.id)).size, items.length);
  for (const group of api.groups) {
    assert.ok(hasChinese(group.title));
    assert.ok(hasChinese(group.description));
    for (const item of group.items) {
      assert.match(item.id, /^[a-z0-9-]+$/);
      assert.ok(['done', 'partial', 'planned'].includes(item.status));
      assert.ok(hasChinese(item.label));
      assert.ok(hasChinese(item.detail));
    }
  }
  const byId = Object.fromEntries(items.map((item) => [item.id, item]));
  for (const id of ['provider-streaming', 'context-compaction', 'session-tree', 'mcp-optional', 'skill-policy', 'hot-plugins', 'remote-facets']) {
    assert.equal(byId[id].status, 'planned', `${id} 尚未实现`);
  }
  for (const id of ['provider-layer', 'mcp-support', 'skill-support', 'validation']) {
    assert.equal(byId[id].status, 'partial', `${id} 仅部分完成`);
  }
});

test('全选生成每项工作内容与验收门槛，三种状态如实保留', () => {
  const api = planApi();
  const data = { target: '完整路线', priority: '功能完整性', selected: allSelected(api) };
  const text = api.generatePlanText(data, { date: '2026-10-07' });
  const scope = text.split('三、已勾选的实施范围')[1].split('四、当前基线')[0];
  for (const group of api.groups) {
    for (const item of group.items) {
      assert.ok(scope.includes(`• ${item.label}（${api.statusLabels[item.status]}）`), `范围缺少 ${item.id} 及状态`);
    }
  }
  assert.equal((scope.match(/工作内容：/g) || []).length, 47);
  assert.equal((scope.match(/验收门槛：/g) || []).length, 47);
  assert.match(text, /待确认/);
  assert.match(text, /不会自动执行任务/);
  assert.match(text, /真实模型需单独配置并验收/);
  assert.match(text, /当前 MCP 仅为已验证方法子集/);
  assert.match(text, /不直接宣称相对 Pi 的固定倍率/);
});

test('少量勾选的范围与未勾选候选分别导出，用户备注保持纯文本', () => {
  const api = planApi();
  const note = '保留原文：<img src=x onerror=alert(1)>\n中文第二行 & 特殊符号';
  const data = { target: '针对路由验收', priority: '可追踪', notes: note, selected: { 'model-routing': true, reasoning: true } };
  const snapshot = JSON.stringify(data);
  const text = api.generatePlanText(data, { date: '2026-10-07' });
  assert.equal(JSON.stringify(data), snapshot, '生成器不能修改用户选择');
  assert.ok(text.includes(note), '备注应保留为原始文本');
  const selected = text.split('三、已勾选的实施范围')[1].split('未勾选范围')[0];
  assert.ok(selected.includes('模型分级与自动路由'));
  assert.ok(selected.includes('Reasoning'));
  assert.equal(selected.includes('MCP OAuth'), false);
  const unselected = text.split('未勾选范围')[1].split('四、当前基线')[0];
  assert.ok(unselected.includes('MCP OAuth'));
  assert.equal(api.computeDependencies(data).length, 0);
});

// 只模拟 docs.js 使用的 DOM 操作；如果源码开始插入原始 HTML，这个 fixture 会立即报错。
class SafeNode {
  constructor(tagName, text = '') {
    this.tagName = tagName;
    this.children = [];
    this._text = String(text);
    this.attributes = {};
    this.events = {};
    this.dataset = {};
    this.value = '';
    this.disabled = false;
    this.checked = false;
    this.hidden = false;
    this.style = {};
  }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
  set innerHTML(_) { throw new Error('测试 DOM 禁止原始 HTML 插入'); }
  append(...nodes) {
    for (const value of nodes) {
      const node = typeof value === 'string' ? new SafeNode('#text', value) : value;
      if (node.tagName === '#fragment') this.children.push(...node.children);
      else this.children.push(node);
    }
  }
  appendChild(node) { this.append(node); return node; }
  replaceChildren(...nodes) { this._text = ''; this.children = []; this.append(...nodes); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  addEventListener(name, listener) { (this.events[name] ||= []).push(listener); }
  dispatch(name) { for (const listener of this.events[name] || []) listener({ target: this, preventDefault() {} }); }
  click() { if (!this.disabled) this.dispatch('click'); }
  remove() {}
}

function documentFixture() {
  const nodes = new Map();
  const created = [];
  const document = {
    title: '',
    body: new SafeNode('body'),
    getElementById(id) {
      if (!nodes.has(id)) { const node = new SafeNode('div'); node.id = id; nodes.set(id, node); }
      return nodes.get(id);
    },
    createElement(tag) { const node = new SafeNode(tag); created.push(node); return node; },
    createTextNode(text) { return new SafeNode('#text', text); },
    createDocumentFragment() { return new SafeNode('#fragment'); },
    addEventListener() {},
  };
  return { document, nodes, created };
}

function descendants(node) { return [node, ...node.children.flatMap(descendants)]; }

test('文档渲染恶意 HTML 与 javascript 链接时保持文本，不创建可执行节点', () => {
  const fixture = documentFixture();
  const window = { SamhoDocuments: [{ id: 'guide', title: '中文说明', category: '开始使用', path: 'docs/guide.md', body: '# 正文' }], addEventListener() {} };
  const sandbox = { window, document: fixture.document, location: { hash: '' } };
  vm.runInNewContext(read('web/docs.js'), sandbox, { filename: 'web/docs.js' });
  const raw = '# 中文标题\n\n<script>alert(1)</script>\n\n[危险](javascript:alert(1))\n\n[安全](https://example.com/说明)\n\n[内部](docs/guide.md)\n\n`<svg onload=alert(1)>`\n\n| 项目 | 说明 |\n| --- | --- |\n| 一个 | 中文 |\n\n```js\n<script>恶意文本</script>\n```';
  const fragment = window.SamhoDocs.renderMarkdown(raw);
  const nodes = descendants(fragment);
  assert.ok(fragment.textContent.includes('<script>alert(1)</script>'));
  assert.ok(fragment.textContent.includes('<svg onload=alert(1)>'));
  assert.equal(nodes.some((node) => ['script', 'svg', 'img', 'iframe'].includes(node.tagName)), false);
  const links = nodes.filter((node) => node.tagName === 'a');
  assert.equal(links.some((node) => String(node.href).startsWith('javascript:')), false);
  const external = links.find((node) => node.href === 'https://example.com/说明');
  assert.ok(external);
  assert.equal(external.rel, 'noopener noreferrer');
  assert.equal(external.target, '_blank');
  assert.ok(links.some((node) => node.href === '#guide'));
  assert.ok(nodes.some((node) => node.tagName === 'table'));
});

function mountedPlanFixture() {
  const fixture = documentFixture();
  const window = {};
  const exported = [];
  const copied = [];
  const sandbox = {
    console, Date, window, document: fixture.document, Blob,
    navigator: { clipboard: { writeText(text) { copied.push(text); return Promise.resolve(); } } },
    URL: { createObjectURL(blob) { exported.push(blob); return `blob:test-${exported.length}`; }, revokeObjectURL() {} },
    setTimeout(callback) { callback(); },
  };
  vm.runInNewContext(read('web/plan.js'), sandbox, { filename: 'web/plan.js' });
  const api = window.SamhoPlan;
  const nodes = new Map();
  const values = {
    'plan-project-name': '本地测试项目', 'plan-target': '中文目标',
    'plan-priority': '可靠性', 'plan-date': '2026-10-07',
    'plan-boundary': '本地验证', 'plan-mode': 'Web 与 TUI 共用服务',
    'plan-notes': '', 'plan-sequence': '', 'plan-models': '', 'plan-budget': '',
    'plan-test-level': '', 'plan-deliverables': '', 'plan-operations': '',
    'plan-preview': '',
  };
  for (const [id, value] of Object.entries(values)) {
    const node = new SafeNode(id === 'plan-preview' ? 'textarea' : 'input'); node.id = id; node.value = value; nodes.set(id, node);
  }
  for (const id of ['plan-log-content', 'plan-selected-count', 'plan-dependency-list', 'plan-message', 'plan-select-all', 'plan-select-recommended', 'plan-clear', 'plan-generate', 'plan-reset', 'plan-copy', 'plan-download']) {
    const node = new SafeNode('button'); node.id = id; nodes.set(id, node);
  }
  nodes.get('plan-copy').disabled = true;
  nodes.get('plan-download').disabled = true;
  const inputs = api.groups.flatMap((group) => group.items.map((item) => {
    const node = new SafeNode('input'); node.value = item.id; node.group = group.id; return node;
  }));
  const platforms = [new SafeNode('input')]; platforms[0].value = 'Windows'; platforms[0].checked = true;
  const scenarios = [new SafeNode('input')]; scenarios[0].value = '计划生成'; scenarios[0].checked = true;
  const root = {
    querySelector(selector) { return selector.startsWith('#') ? nodes.get(selector.slice(1)) || null : null; },
    querySelectorAll(selector) {
      if (selector === 'input[data-plan-item]') return inputs;
      if (selector === 'input, textarea, select') return [...nodes.values(), ...inputs, ...platforms, ...scenarios];
      if (selector === '[data-plan-platform]:checked') return platforms.filter((input) => input.checked);
      if (selector === '[data-plan-scenario]:checked') return scenarios.filter((input) => input.checked);
      const group = /data-plan-group="([^"]+)"/.exec(selector);
      if (group) return inputs.filter((input) => input.group === group[1] && input.checked);
      return [];
    },
  };
  const controller = api.mount(root, { renderGroups: false });
  return { ...fixture, nodes, api, controller, inputs, exported, copied };
}

test('挂载计划页的勾选、清空、生成与复制流程遵守校验', async () => {
  const fixture = mountedPlanFixture();
  const { nodes, inputs, controller, copied } = fixture;
  assert.equal(inputs.filter((input) => input.checked).length, 47);
  assert.equal(controller.collect().target, '中文目标');
  nodes.get('plan-clear').click();
  assert.equal(inputs.some((input) => input.checked), false);
  nodes.get('plan-generate').click();
  assert.match(nodes.get('plan-message').textContent, /至少勾选/);
  assert.equal(nodes.get('plan-download').disabled, true);
  const routing = inputs.find((input) => input.value === 'model-routing');
  routing.checked = true;
  routing.dispatch('change');
  nodes.get('plan-generate').click();
  assert.match(nodes.get('plan-preview').value, /模型分级与自动路由/);
  assert.equal(nodes.get('plan-download').disabled, false);
  assert.equal(nodes.get('plan-copy').disabled, false);
  nodes.get('plan-copy').click();
  await Promise.resolve();
  assert.equal(copied[0], nodes.get('plan-preview').value);
});

test('UTF-8 BOM 下载与预览内容一致，修改字段或选择后旧导出立即失效', async () => {
  const fixture = mountedPlanFixture();
  const { nodes, exported } = fixture;
  nodes.get('plan-generate').click();
  const preview = nodes.get('plan-preview').value;
  nodes.get('plan-download').click();
  assert.equal(exported.length, 1);
  assert.equal(exported[0].type, 'text/plain;charset=utf-8');
  const bytes = new Uint8Array(await exported[0].arrayBuffer());
  assert.deepEqual(Array.from(bytes.slice(0, 3)), [0xef, 0xbb, 0xbf], '下载文件应包含 UTF-8 BOM');
  assert.equal(Buffer.from(bytes.slice(3)).toString('utf8'), preview);
  const anchor = fixture.created.find((node) => node.tagName === 'a' && node.download);
  assert.equal(anchor.download, 'samho-确认执行计划-2026-10-07.txt');
  nodes.get('plan-date').value = '2026-10-08';
  nodes.get('plan-date').dispatch('change');
  assert.equal(nodes.get('plan-download').disabled, true);
  assert.equal(nodes.get('plan-copy').disabled, true);
  assert.match(nodes.get('plan-message').textContent, /重新生成/);
  nodes.get('plan-download').click();
  assert.equal(exported.length, 1, '陈旧计划不能下载');
  nodes.get('plan-generate').click();
  assert.match(nodes.get('plan-preview').value, /生成日期：2026-10-08/);
  nodes.get('plan-select-recommended').click();
  assert.equal(nodes.get('plan-download').disabled, true);
});
