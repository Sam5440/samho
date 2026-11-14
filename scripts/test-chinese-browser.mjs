// 使用本机 Chrome 和 Node 22 内置 CDP 客户端；不调用任务或模型接口。
// node scripts/test-chinese-browser.mjs http://127.0.0.1:374
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const base = (process.argv[2] || 'http://127.0.0.1:374').replace(/\/$/, '');
assert.ok(/^http:\/\/(127\.0\.0\.1|localhost):\d+$/.test(base), '仅允许本机服务地址');
const basePort = Number(new URL(base).port);
assert.ok(basePort >= 300 && basePort <= 400, '测试服务端口必须在 300-400');
const reportDir = path.join(root, 'reports', 'chinese-pages');
await fs.mkdir(reportDir, { recursive: true });
const chromeCandidates = [process.env.SAMHO_CHROME, 'C:/Program Files/Google/Chrome/Application/chrome.exe', '/usr/bin/google-chrome', '/usr/bin/chromium'].filter(Boolean);
let chromePath;
for (const candidate of chromeCandidates) {
  try { await fs.access(candidate); chromePath = candidate; break; } catch {}
}
assert.ok(chromePath, '未找到 Chrome；可通过 SAMHO_CHROME 指定浏览器路径');
const profile = await fs.mkdtemp(path.join(os.tmpdir(), 'samho-chinese-browser-'));
const cdpPort = Number(process.env.SAMHO_CDP_PORT || 390);
assert.ok(cdpPort >= 300 && cdpPort <= 400, 'CDP 测试端口必须在 300-400');
const browser = spawn(chromePath, ['--headless=new', `--remote-debugging-port=${cdpPort}`, '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--disable-extensions', '--disable-component-update', `--user-data-dir=${profile}`, 'about:blank'], { windowsHide: true, stdio: 'ignore' });
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check, description, timeout = 15000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await check();
    if (result) return result;
    await pause(50);
  }
  throw new Error('超时：' + description);
}
class Cdp {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.id = 0;
    this.pending = new Map();
    this.events = [];
    this.ready = new Promise((resolve, reject) => {
      this.socket.addEventListener('open', resolve, { once: true });
      this.socket.addEventListener('error', reject, { once: true });
    });
    this.socket.addEventListener('message', ({ data }) => {
      const message = JSON.parse(data);
      if (!message.id) { this.events.push(message); return; }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      clearTimeout(pending.timer);
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
      else pending.resolve(message.result);
    });
  }
  async send(method, params = {}) {
    await this.ready;
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error('CDP 超时：' + method)); }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  async evaluate(expression) {
    const result = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
}
const report = { generated_at: new Date().toISOString(), base, checks: [], viewports: [], requests: [], errors: [], status: 'running' };
let cdp;
let downloadedPath;
try {
  // include_str! 资源必须来自最终文件，避免旧二进制掩盖界面/文档修改。
  // 工作台为 React 构建产物（web/dist），文档/计划页为 web/ 下的独立静态页。
  for (const [route, source] of [['/', 'dist/index.html'], ['/index.html', 'dist/index.html'], ['/docs', 'docs.html'], ['/docs.html', 'docs.html'], ['/plan', 'plan.html'], ['/plan.html', 'plan.html'], ['/plan.js', 'plan.js'], ['/plan.css', 'plan.css'], ['/docs.js', 'docs.js'], ['/docs.css', 'docs.css'], ['/documents-data.js', 'documents-data.js'], ['/assets/app.js', 'dist/assets/app.js'], ['/assets/app.css', 'dist/assets/app.css']]) {
    const response = await fetch(base + route);
    assert.equal(response.status, 200, route);
    assert.match(response.headers.get('content-type'), /charset=utf-8/i);
    assert.equal(await response.text(), await fs.readFile(path.join(root, 'web', source), 'utf8'), '服务与本地文件一致：' + route);
    report.checks.push({ name: 'UTF-8 静态路由与最终文件一致：' + route, passed: true });
  }
  await until(async () => {
    try { return (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok; } catch { return false; }
  }, 'Chrome 启动');
  const target = await (await fetch(`http://127.0.0.1:${cdpPort}/json/new?about:blank`, { method: 'PUT' })).json();
  cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  await cdp.send('Network.enable');
  await cdp.send('Log.enable');
  await cdp.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: reportDir, eventsEnabled: true });
  async function navigate(url, ready) {
    await cdp.send('Page.navigate', { url });
    await until(() => cdp.evaluate(`document.readyState === 'complete' && (${ready})`), url);
  }
  async function check(name, expression) {
    const value = await cdp.evaluate(expression);
    assert.ok(value, name);
    report.checks.push({ name, passed: true });
  }
  await navigate(base + '/plan', 'Boolean(window.SamhoPlan)');
  await check('服务页面显示 47 项，默认全选', "document.querySelectorAll('input[data-plan-item]:checked').length === 47");
  await cdp.evaluate("document.getElementById('plan-clear').click(); document.getElementById('plan-generate').click()");
  await check('空范围禁止导出', "document.getElementById('plan-download').disabled && document.getElementById('plan-message').textContent.includes('至少勾选')");
  await cdp.evaluate("const i=document.querySelector('input[value=\"web-gui\"]'); i.checked=true; i.dispatchEvent(new Event('change', {bubbles:true}))");
  await check('单选缺依赖时提示，保留原选择', "document.querySelectorAll('input[data-plan-item]:checked').length === 1 && document.getElementById('plan-dependency-list').textContent.includes('shared-runtime')");
  await cdp.evaluate("document.getElementById('plan-select-recommended').click(); document.getElementById('plan-generate').click()");
  await check('推荐首期生成完整中文计划', "['未勾选范围','当前基线','依赖与风险提示','回传确认模板','待确认'].every(s => document.getElementById('plan-preview').value.includes(s))");
  await cdp.evaluate("const n=document.getElementById('plan-notes'); n.value='<img src=x onerror=window.planInjected=true>中文备注'; n.dispatchEvent(new Event('input', {bubbles:true}))");
  await check('修改后旧导出失效', "document.getElementById('plan-copy').disabled && document.getElementById('plan-download').disabled");
  await cdp.evaluate("document.getElementById('plan-generate').click()");
  await check('用户备注为纯文本', "!window.planInjected && !document.querySelector('img[src=x]') && document.getElementById('plan-preview').value.includes('<img src=x onerror=window.planInjected=true>中文备注')");
  // 把浏览器生成的实际 Blob 下载到报告目录，逐字核对 UTF-8 BOM 和预览。
  const expected = await cdp.evaluate("({filename: SamhoPlan.filename(SamhoPlan.collect(document.querySelector('[data-samho-plan-generator]'))), text: document.getElementById('plan-preview').value})");
  downloadedPath = path.join(reportDir, expected.filename);
  try { await fs.unlink(path.join(reportDir, expected.filename)); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  await cdp.evaluate("document.getElementById('plan-download').click()");
  const downloaded = await until(async () => {
    try { return await fs.readFile(path.join(reportDir, expected.filename)); } catch { return false; }
  }, '计划文件下载');
  assert.deepEqual([...downloaded.subarray(0, 3)], [0xef, 0xbb, 0xbf]);
  assert.equal(downloaded.subarray(3).toString('utf8'), expected.text);
  report.checks.push({ name: '真实下载文件为 UTF-8 BOM 且与预览逐字一致', passed: true, filename: expected.filename, bytes: downloaded.length });
  await cdp.evaluate("document.getElementById('plan-select-all').click(); document.getElementById('plan-generate').click()");
  await check('全选生成后保留已完成/部分完成/未完成状态', "['已完成（首版已验证）','部分完成（有明确边界）','未完成（确认后实施）'].every(s => document.getElementById('plan-preview').value.includes(s))");
  for (const [width, height] of [[1710, 934], [560, 900], [375, 900]]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
    await cdp.evaluate('window.scrollTo(0,0)');
    await pause(100);
    const dimensions = await cdp.evaluate('({width:innerWidth,client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})');
    assert.ok(dimensions.scroll <= dimensions.client, `计划页 ${width} 无横向溢出`);
    report.viewports.push({ page: 'plan', ...dimensions, height });
    const screenshot = await cdp.send('Page.captureScreenshot', { format: 'png' });
    await fs.writeFile(path.join(reportDir, `plan-${width}.png`), Buffer.from(screenshot.data, 'base64'));
  }
  await navigate(base + '/docs', 'Boolean(window.SamhoDocuments && window.SamhoDocs)');
  await check('13 份中文文档与代码块可阅读', "SamhoDocuments.length === 13 && document.querySelectorAll('#docs-body pre code').length >= 4");
  await cdp.evaluate("const s=document.getElementById('docs-search'); s.value='MCP'; s.dispatchEvent(new Event('input',{bubbles:true}))");
  await check('搜索中文资料有效', "document.querySelectorAll('#docs-list button').length > 0 && !document.getElementById('docs-count').textContent.startsWith('0 /')");
  for (const doc of await cdp.evaluate('SamhoDocuments.map(d=>({id:d.id,title:d.title}))')) {
    await cdp.evaluate(`location.hash=${JSON.stringify(doc.id)}`);
    await until(() => cdp.evaluate(`document.getElementById('docs-title').textContent === ${JSON.stringify(doc.title)}`), doc.id);
    await check('文档可渲染：' + doc.title, "document.getElementById('docs-body').textContent.length > 100");
  }
  const dimensions = await cdp.evaluate('({width:innerWidth,client:document.documentElement.clientWidth,scroll:document.documentElement.scrollWidth})');
  assert.ok(dimensions.scroll <= dimensions.client, '文档页窄屏无横向溢出');
  report.viewports.push({ page: 'docs', ...dimensions, height: 900 });
  await cdp.evaluate("location.hash='guide';window.scrollTo(0,0)");
  await pause(100);
  const screenshot = await cdp.send('Page.captureScreenshot', { format: 'png' });
  await fs.writeFile(path.join(reportDir, 'docs-375.png'), Buffer.from(screenshot.data, 'base64'));
  // 直接打开文件，验证页面不依赖 Rust 服务或 HTTP API。
  await navigate(pathToFileURL(path.join(root, 'web', 'plan.html')).href, 'Boolean(window.SamhoPlan)');
  await cdp.evaluate("document.getElementById('plan-generate').click()");
  await check('计划页支持 file:// 离线生成', "document.getElementById('plan-preview').value.includes('回传确认模板') && !document.getElementById('plan-download').disabled");
  await navigate(pathToFileURL(path.join(root, 'web', 'docs.html')).href, 'Boolean(window.SamhoDocuments && window.SamhoDocs)');
  await check('文档页支持 file:// 离线阅读', "SamhoDocuments.length===13 && document.getElementById('docs-body').textContent.includes('中文使用说明')");
  report.requests = cdp.events.filter(e=>e.method==='Network.requestWillBeSent').map(e=>({url:e.params.request.url,method:e.params.request.method,type:e.params.type}));
  assert.ok(!report.requests.some(r=>/\/api\//.test(r.url)), 'docs/plan 不调用任何 API');
  report.errors = cdp.events.filter(e=>e.method==='Runtime.exceptionThrown' || e.method==='Log.entryAdded' && ['error','warning'].includes(e.params.entry.level));
  assert.equal(report.errors.length, 0, '浏览器无错误或警告');
  report.checks.push({ name: '全部操作无 API 请求与控制台错误/警告', passed: true });
  report.status = 'passed';
} catch (error) {
  report.status = 'failed';
  report.failure = error.stack || String(error);
  process.exitCode = 1;
} finally {
  if (cdp) {
    report.requests = cdp.events.filter(e=>e.method==='Network.requestWillBeSent').map(e=>({url:e.params.request.url,method:e.params.request.method,type:e.params.type}));
    report.errors = cdp.events.filter(e=>e.method==='Runtime.exceptionThrown' || e.method==='Log.entryAdded' && ['error','warning'].includes(e.params.entry.level));
  }
  await fs.writeFile(path.join(reportDir, 'browser-validation.json'), JSON.stringify(report, null, 2), 'utf8');
  if (cdp) { try { await cdp.send('Browser.close'); } catch {} cdp.socket.close(); }
  if (browser.exitCode === null) await Promise.race([new Promise(resolve=>browser.once('exit', resolve)), pause(3000)]);
  if (browser.exitCode === null) browser.kill();
  // 浏览器退出后再删测试下载，防止浏览器尚在写入时留下空占位文件。
  if (downloadedPath && path.dirname(downloadedPath) === reportDir) {
    try { await fs.unlink(downloadedPath); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  // 删除范围限定在本次 mkdtemp 产生的测试目录，不触及用户浏览器配置。
  const cleanup = path.resolve(profile);
  assert.ok(path.dirname(cleanup) === path.resolve(os.tmpdir()) && path.basename(cleanup).startsWith('samho-chinese-browser-'));
  try { await fs.rm(cleanup, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); } catch {}
}
console.log(JSON.stringify({ status: report.status, checks: report.checks.length, report: path.join(reportDir, 'browser-validation.json'), failure: report.failure }, null, 2));
