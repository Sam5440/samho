// 临时脚本：对前端模块做真实 ESM 语法检查（动态 import；运行期 DOM 缺失不算语法错误）。
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

const files = [
  'web/js/api.js',
  'web/js/workspaces.js',
  'web/js/panels/models.js',
  'web/js/panels/agents.js',
  'web/js/panels/tasks.js',
  'web/js/main.js',
  'web/js/composer.js',
  'web/js/model-select.js',
  'web/js/dock.js',
  'web/js/sidebar.js',
  'web/js/store.js',
  'web/js/ui.js',
  'web/js/projects-tree.js',
  'web/js/projects.js',
  'web/js/sessions.js',
  'web/js/panels/terminal.js',
  'web/js/panels/browser.js',
];
let bad = 0;
for (const file of files) {
  try {
    await import(pathToFileURL(resolve(file)).href);
    console.log('OK  ', file);
  } catch (error) {
    if (error instanceof SyntaxError || /Unexpected token|import statement/.test(error.message)) {
      console.log('SYNTAX FAIL', file, error.message);
      bad += 1;
    } else {
      console.log('OK  ', file, `(runtime: ${error.message.slice(0, 48)})`);
    }
  }
}
process.exit(bad ? 1 : 0);
