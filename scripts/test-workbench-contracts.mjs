// 工作台回归契约（静态断言，风格与 test-chinese-pages.mjs 一致）：
// 轨迹搜索不得使用 innerHTML；live 正文不进 aria-live；移动端侧栏抽屉可达；
// 工作区切换代际守卫与任务提交锁存在。
import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (relative) => fs.readFileSync(path.join(root, relative), 'utf8');

test('轨迹搜索高亮只使用文本节点，不经 innerHTML 注入', () => {
  const source = read('web/js/trajectory/render.js');
  assert.equal(source.includes('innerHTML'), false, '轨迹面板不得使用 innerHTML（trace 正文不可信）');
  assert.match(source, /createElement\('mark'\)/, '命中区间应使用 <mark> 元素');
  assert.match(source, /mark\.textContent/, 'mark 内容必须走 textContent');
  assert.doesNotMatch(source, /<mark>/, '不得拼接 HTML 字符串');
});

test('轨迹面板支持增量追加与末卡刷新', () => {
  const source = read('web/js/trajectory/render.js');
  assert.match(source, /appendEvents/, 'SSE/分页需要增量追加入口');
  assert.match(source, /refreshTail/, 'model.response 只需重建末卡');
  assert.match(source, /seenEvents/, '追加事件需要身份去重');
});

test('工作区切换与加载有代际守卫', () => {
  const source = read('web/js/main.js');
  assert.match(source, /workspaceGen/, '需要工作区加载代际计数');
  assert.match(source, /gen !== workspaceGen/, '响应返回前必须校验代际');
  assert.match(source, /currentWorkspaceId !== target/, '切换收尾前必须校验当前工作区');
});

test('任务提交在 /api/run 返回前有提交锁', () => {
  const main = read('web/js/main.js');
  const composer = read('web/js/composer.js');
  assert.match(main, /let submitting = false/, 'main 提交锁');
  assert.match(composer, /submitInFlight/, 'composer 提交锁');
  assert.match(composer, /!textarea\.value\.trim\(\)/, '失败回填不得覆盖用户新输入');
});

test('移动端提供可访问的侧栏抽屉入口', () => {
  const html = read('web/index.html');
  const css = read('web/app.css');
  assert.match(html, /id="toggle-sidebar"/);
  assert.match(html, /aria-controls="sidebar"/);
  assert.match(html, /id="sidebar-overlay"/);
  assert.match(css, /body\.sidebar-open \.sidebar/, '打开态需要显示抽屉');
  assert.doesNotMatch(css, /\.sidebar \{ display: none; \}/, '窄屏不得整体隐藏侧栏');
});

test('流式正文不再作为 aria-live 区域整段重播', () => {
  const source = read('web/js/timeline/render.js');
  assert.doesNotMatch(
    source,
    /class: 'live-turn', 'aria-live'/,
    'live turn 容器不得声明 aria-live',
  );
  assert.match(source, /announce/, '需要 sr-only 状态播报入口');
  assert.match(source, /role: 'status'/, '播报节点应为 role=status');
});

test('项目制侧栏：项目树不使用 innerHTML，附属行默认折叠', () => {
  const tree = read('web/js/projects-tree.js');
  assert.equal(tree.includes('innerHTML'), false, '项目树不得使用 innerHTML');
  assert.match(tree, /attachOpenSet/, '附属行展开状态集合');
  // 默认折叠：只有点击 toggle 后加入 attachOpenSet 才渲染 attachRow。
  assert.match(tree, /attachOpenSet\.has\(id\)/, '渲染前必须检查展开状态');
  assert.match(tree, /projectOpenState/, '项目展开状态由后端 open 字段兜底');
  assert.match(tree, /setAttachPanel/, '附属面板桥必须可注入');
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
