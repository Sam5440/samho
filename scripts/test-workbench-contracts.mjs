// 工作台回归契约（静态断言）：
// React + shadcn/ui 前端（web/app）下，轨迹搜索不得经 HTML 注入；live 正文不进 aria-live；
// 移动端侧栏抽屉可达；工作区切换代际守卫与任务提交锁存在；构建产物满足 CSP。
import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');
const src = (relative) => read(`web/app/src/${relative}`);

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    return entry.isDirectory() ? walk(full) : [full];
  });
}

test('轨迹搜索高亮只使用文本节点与 <mark> 元素，不经 HTML 注入', () => {
  const source = src('components/panels/trajectory.tsx');
  assert.equal(source.includes('innerHTML'), false, '轨迹面板不得使用 innerHTML / dangerouslySetInnerHTML（trace 正文不可信）');
  assert.match(source, /<mark key=/, '命中区间应使用 <mark> 元素');
  assert.doesNotMatch(source, /'<mark>'|"<mark>"|`<mark>/, '不得拼接 HTML 字符串');
});

test('只有净化后的 markdown 输出允许走 dangerouslySetInnerHTML', () => {
  const files = walk(path.join(root, 'web/app/src')).filter((f) => /\.(tsx?|jsx?)$/.test(f));
  const injectors = [];
  for (const file of files) {
    const text = fs.readFileSync(file, 'utf8');
    assert.doesNotMatch(text, /\.innerHTML\s*=|outerHTML|insertAdjacentHTML|new Function\(|eval\(/, `${path.relative(root, file)} 不得使用原始 HTML/动态代码注入`);
    for (const match of text.matchAll(/dangerouslySetInnerHTML=\{\{ __html: ([^}]+) \}\}/g)) {
      assert.equal(match[1].trim(), 'html', `${path.relative(root, file)} 只能注入 renderMarkdown 的结果`);
      assert.match(text, /renderMarkdown\(/, `${path.relative(root, file)} 注入的 html 必须来自 renderMarkdown`);
      injectors.push(path.relative(root, file).split(path.sep).join('/'));
    }
  }
  assert.deepEqual(injectors, ['web/app/src/components/markdown.tsx'], 'HTML 注入只允许集中在 Markdown 组件');
  const markdown = src('lib/markdown.ts');
  assert.match(markdown, /DOMPurify\.sanitize\(/, 'markdown 输出必须经 DOMPurify 净化');
  const forbidTags = markdown.match(/const FORBID_TAGS = \[([^\]]*)\]/)?.[1] || '';
  for (const tag of ['script', 'style', 'iframe', 'object', 'embed', 'form', 'base', 'meta']) {
    assert.ok(forbidTags.includes(`'${tag}'`), `净化必须禁止 <${tag}>`);
  }
  assert.match(markdown, /FORBID_ATTR = \[[^\]]*'style'/, '净化必须剥离 style 属性（CSP style-src self）');
});

test('可执行 HTML 只在沙箱预览中运行', () => {
  const dialog = src('components/preview-dialog.tsx');
  assert.match(dialog, /sandbox=\{SANDBOX\}/, '预览 iframe 必须声明 sandbox');
  assert.doesNotMatch(dialog, /allow-same-origin/, '沙箱不得与工作台同源');
  const preview = read('crates/samho-cli/src/preview.rs');
  assert.match(preview, /PREVIEW_CSP: &str = "sandbox allow-scripts/, '服务端预览响应自带 sandbox CSP');
  assert.doesNotMatch(preview.match(/PREVIEW_CSP[^;]*;/)[0], /allow-same-origin/, '服务端沙箱不得同源');
  assert.match(preview, /connect-src 'none'/, '预览文档禁止网络请求');
});

test('轨迹面板支持增量追加与身份去重', () => {
  const controller = src('lib/controller.ts');
  assert.match(controller, /export function appendEvents/, 'SSE/分页需要增量追加入口');
  assert.match(controller, /seenEvents/, '追加事件需要身份去重');
  assert.match(controller, /trajectoryLoads/, '同一 trace 的分页加载需要去重');
});

test('工作区切换与加载有代际守卫', () => {
  const source = src('lib/controller.ts');
  assert.match(source, /workspaceGen/, '需要工作区加载代际计数');
  assert.match(source, /gen !== workspaceGen/, '响应返回前必须校验代际');
  assert.match(source, /currentWorkspaceId !== target/, '切换收尾前必须校验当前工作区');
});

test('任务提交在 /api/run 返回前有提交锁', () => {
  const controller = src('lib/controller.ts');
  const composer = src('components/composer.tsx');
  assert.match(controller, /let submitting = false/, 'controller 提交锁');
  assert.match(composer, /submitInFlight/, 'composer 提交锁');
  assert.match(composer, /!textarea\.value\.trim\(\)/, '失败回填不得覆盖用户新输入');
});

test('移动端提供可访问的侧栏抽屉入口', () => {
  const topbar = src('components/topbar.tsx');
  const app = src('App.tsx');
  assert.match(topbar, /id="toggle-sidebar"/);
  assert.match(topbar, /aria-controls="sidebar"/);
  assert.match(app, /<Sheet open=\{sidebarOpen\}/, '窄屏侧栏改为抽屉而非隐藏');
  assert.match(app, /side="left" id="sidebar"/);
});

test('流式正文不作为 aria-live 区域整段重播', () => {
  const conversation = src('components/conversation.tsx');
  const app = src('App.tsx');
  assert.doesNotMatch(conversation, /aria-live=/, '时间线与 live turn 不得声明 aria-live');
  assert.match(app, /role="status" aria-live="polite">\{announcement\}/, '需要 sr-only 状态播报节点');
  assert.match(src('lib/controller.ts'), /announce\(`任务状态：/, '只播报阶段摘要');
});

test('项目制侧栏：附属行默认折叠，项目展开状态由后端 open 字段兜底', () => {
  const tree = src('components/sidebar.tsx');
  assert.equal(tree.includes('innerHTML'), false, '项目树不得使用 innerHTML');
  assert.match(tree, /attachOpenSet/, '附属行展开状态集合');
  assert.match(tree, /attachOpenSet\.has\(session\.id\)/, '渲染前必须检查展开状态');
  assert.match(tree, /projectOpenState/, '项目展开状态由后端 open 字段兜底');
  assert.match(src('lib/bridge.ts'), /terminal: AttachHandle \| null/, '附属面板桥必须可注入');
});

test('CSP 兼容：不引入运行期注入 <style> 的依赖，滚动锁被垫片替换', () => {
  const pkg = JSON.parse(read('web/app/package.json'));
  const deps = { ...pkg.dependencies, ...pkg.devDependencies };
  for (const banned of ['sonner', 'vaul', 'next-themes', '@radix-ui/react-select', 'styled-components', '@emotion/react']) {
    assert.equal(banned in deps, false, `${banned} 会运行期注入样式或不适用`);
  }
  assert.match(read('web/app/vite.config.ts'), /'react-remove-scroll':/, 'react-remove-scroll 必须被别名为垫片');
  const html = read('web/dist/index.html');
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/, '构建产物不得含内联脚本');
  assert.doesNotMatch(html, /<style/, '构建产物不得含内联样式');
  assert.match(html, /src="\/assets\/app\.js"/, '入口文件名固定，供 include_str! 内嵌');
  assert.match(html, /href="\/assets\/app\.css"/);
});

test('构建产物存在且被服务端内嵌', () => {
  const server = read('crates/samho-cli/src/server.rs');
  for (const asset of ['web/dist/index.html', 'web/dist/assets/app.js', 'web/dist/assets/app.css']) {
    assert.ok(fs.existsSync(path.join(root, asset)), `${asset} 缺失：先运行 npm --prefix web/app run build`);
    assert.ok(server.includes(`../../../${asset}`), `server.rs 必须内嵌 ${asset}`);
  }
});

test('后端项目制 API 契约：项目 patch 支持 folders/entry/open，附属面板可持久化', () => {
  const server = read('crates/samho-cli/src/server.rs');
  assert.match(server, /session_attachments_get/, 'GET attachments handler');
  assert.match(server, /session_attachments_put/, 'PUT attachments handler');
  assert.match(server, /validate_project_path/, '文件夹路径需 confined 复核');
  const term = read('crates/samho-cli/src/term.rs');
  assert.match(term, /list_by_session/, 'term list by session');
  const projects = read('crates/samho-core/src/projects.rs');
  assert.match(projects, /pub folders/, 'Project.folders 字段');
  assert.match(projects, /pub entry/, 'Project.entry 字段');
  assert.match(read('crates/samho-core/src/agent.rs'), /migrate_default_project/, '默认项目迁移');
});
