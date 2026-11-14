// 前端构建检查：web/app 的 TypeScript 由 `npm --prefix web/app run build`（tsc + vite）负责；
// 这里对内嵌进二进制的构建产物做语法校验（只解析不执行，不依赖 DOM）。缺失文件视为失败。
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import vm from 'node:vm';

const files = ['web/dist/assets/app.js'];
let bad = 0;
for (const file of files) {
  const full = resolve(file);
  if (!existsSync(full)) {
    console.log('MISSING', file, '（先运行 npm --prefix web/app run build）');
    bad += 1;
    continue;
  }
  try {
    // Vite 产物为单文件模块且无顶层 import/export，可直接按 Script 解析。
    const source = readFileSync(full, 'utf8');
    if (/^\s*(import|export)\s/m.test(source)) throw new Error('产物含顶层 import/export，可能发生了代码分割');
    new vm.Script(source, { filename: file });
    console.log('OK  ', file);
  } catch (error) {
    console.log('FAIL', file, error.message);
    bad += 1;
  }
}
process.exit(bad ? 1 : 0);
