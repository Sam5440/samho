// Markdown 渲染：markdown-it（CommonMark + GFM 表格/删除线/自动链接 + 原始 HTML）
// → highlight.js 代码高亮 → Temml 数学公式（MathML）→ DOMPurify 净化。
// 净化是唯一的安全边界：脚本、事件属性、内联样式（CSP 本就禁止）、框架/表单类标签一律剥离；
// 需要真正执行的 HTML（网页/小游戏）走沙箱预览（/api/preview，独立 CSP + 不透明源）。

import MarkdownIt, { type StateBlock, type StateCore, type StateInline } from 'markdown-it';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import glsl from 'highlight.js/lib/languages/glsl';
import go from 'highlight.js/lib/languages/go';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import kotlin from 'highlight.js/lib/languages/kotlin';
import lua from 'highlight.js/lib/languages/lua';
import markdown from 'highlight.js/lib/languages/markdown';
import php from 'highlight.js/lib/languages/php';
import plaintext from 'highlight.js/lib/languages/plaintext';
import powershell from 'highlight.js/lib/languages/powershell';
import python from 'highlight.js/lib/languages/python';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import scss from 'highlight.js/lib/languages/scss';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import swift from 'highlight.js/lib/languages/swift';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import temml from 'temml';
import { apiPath } from '@/lib/api';

const LANGUAGES = {
  bash, c, cpp, csharp, css, diff, dockerfile, glsl, go, ini, java, javascript, json, kotlin, lua, markdown,
  php, plaintext, powershell, python, ruby, rust, scss, shell, sql, swift, typescript, xml, yaml,
};
for (const [name, language] of Object.entries(LANGUAGES)) hljs.registerLanguage(name, language);

/** 可在沙箱中预览运行的代码块语言。 */
export const PREVIEWABLE_LANGS = new Set(['html', 'htm', 'xhtml', 'svg']);

const escapeHtml = (text: string) =>
  text.replace(/[&<>"']/g, (ch) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch]!);

// ---------- 数学公式：$…$ / $$…$$ / \(…\) / \[…\] ----------

function renderMath(tex: string, displayMode: boolean) {
  try {
    return temml.renderToString(tex, { displayMode, throwOnError: false });
  } catch {
    return `<code class="md-math-error">${escapeHtml(tex)}</code>`;
  }
}

function mathInline(state: StateInline, silent: boolean) {
  const src = state.src;
  const start = state.pos;
  let open: string;
  let close: string;
  if (src.startsWith('$$', start)) { open = '$$'; close = '$$'; }
  else if (src[start] === '$') { open = '$'; close = '$'; }
  else if (src.startsWith('\\(', start)) { open = '\\('; close = '\\)'; }
  else return false;
  const contentStart = start + open.length;
  // 单个 $：开头不能是空白，避免把“$5 和 $10”误判为公式。
  if (open === '$' && (!src[contentStart] || /\s/.test(src[contentStart]))) return false;
  let end = contentStart;
  for (;;) {
    end = src.indexOf(close, end);
    if (end === -1) return false;
    if (src[end - 1] === '\\' && open !== '\\(') { end += 1; continue; }
    break;
  }
  if (end === contentStart) return false;
  if (open === '$' && (/\s/.test(src[end - 1]) || /\d/.test(src[end + 1] || ''))) return false;
  if (!silent) {
    const token = state.push('math_inline', 'math', 0);
    token.content = src.slice(contentStart, end);
    token.markup = open;
  }
  state.pos = end + close.length;
  return true;
}

function mathBlock(state: StateBlock, startLine: number, endLine: number, silent: boolean) {
  const lineText = (line: number) => state.src.slice(state.bMarks[line] + state.tShift[line], state.eMarks[line]);
  const first = lineText(startLine);
  const pair = first.startsWith('$$') ? ['$$', '$$'] : first.startsWith('\\[') ? ['\\[', '\\]'] : null;
  if (!pair || state.sCount[startLine] - state.blkIndent >= 4) return false;
  const [open, close] = pair;
  let body = first.slice(open.length);
  let line = startLine;
  let found = false;
  if (body.trimEnd().endsWith(close)) {
    body = body.trimEnd().slice(0, -close.length);
    found = true;
  }
  const parts = [body];
  while (!found && ++line < endLine) {
    const text = lineText(line);
    if (text.trimEnd().endsWith(close)) {
      parts.push(text.trimEnd().slice(0, -close.length));
      found = true;
    } else parts.push(text);
  }
  if (!found) return false;
  if (silent) return true;
  const token = state.push('math_block', 'math', 0);
  token.block = true;
  token.content = parts.join('\n').trim();
  token.map = [startLine, line + 1];
  token.markup = open;
  state.line = line + 1;
  return true;
}

// ---------- GFM 任务列表：- [ ] / - [x] ----------

function taskLists(state: StateCore) {
  const tokens = state.tokens;
  for (let i = 2; i < tokens.length; i++) {
    const inline = tokens[i];
    if (inline.type !== 'inline' || tokens[i - 1].type !== 'paragraph_open' || tokens[i - 2].type !== 'list_item_open') continue;
    const match = inline.content.match(/^\[([ xX])\]\s/);
    const firstChild = inline.children?.[0];
    if (!match || !firstChild || firstChild.type !== 'text') continue;
    firstChild.content = firstChild.content.slice(match[0].length);
    const box = new state.Token('html_inline', '', 0);
    box.content = `<input type="checkbox" disabled${match[1] === ' ' ? '' : ' checked'}> `;
    inline.children!.unshift(box);
    tokens[i - 2].attrJoin('class', 'md-task');
  }
}

// ---------- markdown-it 实例 ----------

const md = new MarkdownIt({ html: true, linkify: true, breaks: true, typographer: false });
md.inline.ruler.before('escape', 'math_inline', mathInline);
md.block.ruler.before('fence', 'math_block', mathBlock, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
md.core.ruler.after('inline', 'task_lists', taskLists);
md.renderer.rules.math_inline = (tokens, idx) => renderMath(tokens[idx].content, false);
md.renderer.rules.math_block = (tokens, idx) => `<div class="md-math">${renderMath(tokens[idx].content, true)}</div>`;
md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  const lang = token.info.trim().split(/\s+/)[0]?.toLowerCase() || '';
  const code = token.content.replace(/\n$/, '');
  const known = lang && hljs.getLanguage(lang);
  let body: string;
  try {
    body = known ? hljs.highlight(code, { language: lang, ignoreIllegals: true }).value : escapeHtml(code);
  } catch {
    body = escapeHtml(code);
  }
  const actions = (PREVIEWABLE_LANGS.has(lang) ? '<button type="button" data-md-action="preview">预览</button>' : '')
    + '<button type="button" data-md-action="copy">复制</button>';
  return `<div class="md-code-block" data-lang="${escapeHtml(lang)}">`
    + `<div class="md-code-head"><span class="md-code-lang">${escapeHtml(lang || 'text')}</span><span class="md-code-actions">${actions}</span></div>`
    + `<pre><code class="hljs">${body}</code></pre></div>`;
};
md.renderer.rules.table_open = () => '<div class="md-table-wrap"><table>';
md.renderer.rules.table_close = () => '</table></div>';

// ---------- 净化 ----------

const FORBID_TAGS = ['style', 'script', 'iframe', 'frame', 'frameset', 'object', 'embed', 'form', 'base', 'link', 'meta', 'title', 'template', 'textarea', 'select', 'noscript'];
const FORBID_ATTR = ['style', 'srcset', 'action', 'formaction', 'autofocus'];
const EXTERNAL = /^(https?:|mailto:|tel:|data:image\/)/i;

let resolveBase = '';
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  const el = node as Element;
  if (el.tagName === 'A') {
    const href = el.getAttribute('href') || '';
    if (href && !href.startsWith('#') && !EXTERNAL.test(href)) {
      // workspace 相对路径：交给文件面板打开，而不是在工作台内导航。
      el.setAttribute('data-md-file', joinPath(resolveBase, href));
      el.setAttribute('href', '#');
    } else if (/^https?:/i.test(href)) {
      el.setAttribute('target', '_blank');
      el.setAttribute('rel', 'noopener noreferrer');
    }
  }
  if (el.tagName === 'IMG') {
    const src = el.getAttribute('src') || '';
    if (src && !EXTERNAL.test(src)) el.setAttribute('src', apiPath(`/api/file/raw?path=${encodeURIComponent(joinPath(resolveBase, src))}`));
    el.setAttribute('loading', 'lazy');
  }
  if (el.tagName === 'INPUT') el.setAttribute('disabled', '');
});

function joinPath(base: string, relative: string) {
  const clean = relative.split(/[?#]/)[0].replace(/\\/g, '/');
  if (clean.startsWith('/')) return clean.replace(/^\/+/, '');
  const parts = base ? base.split('/') : [];
  for (const seg of clean.split('/')) {
    if (!seg || seg === '.') continue;
    if (seg === '..') parts.pop();
    else parts.push(decodeURIComponentSafe(seg));
  }
  return parts.join('/');
}

function decodeURIComponentSafe(value: string) {
  try { return decodeURIComponent(value); } catch { return value; }
}

// DOMPurify 在继承页面 CSP 的文档里解析 HTML，style 属性在解析阶段就会触发
// style-src 违规上报；在解析前从真实标签上去掉（文本中的 < 已被转义，不会误伤正文）。
const TAG = /<[a-zA-Z][^>]*>/g;
const STYLE_ATTR = /\s+style\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)/gi;
const stripStyleAttributes = (html: string) => html.replace(TAG, (tag) => tag.replace(STYLE_ATTR, ''));

/**
 * 渲染 Markdown 为净化后的 HTML。
 * @param baseDir 相对链接/图片的解析目录（workspace 相对，默认根目录）。
 */
export function renderMarkdown(text: unknown, baseDir = '') {
  const source = String(text ?? '');
  if (!source.trim()) return '';
  resolveBase = baseDir.replace(/^\/+|\/+$/g, '');
  try {
    return DOMPurify.sanitize(stripStyleAttributes(md.render(source)), { FORBID_TAGS, FORBID_ATTR });
  } finally {
    resolveBase = '';
  }
}

// 从推理文本提取“当前正在想什么”的标题：ATX 标题 / 粗体行 / setext 标题，取最靠后者。
export function reasoningHeading(text: unknown) {
  const source = String(text ?? '');
  if (!source.trim()) return '';
  let best = '';
  let bestAt = -1;
  const consider = (match: string, offset: number) => {
    if (match && offset >= bestAt) { best = match.trim(); bestAt = offset; }
  };
  let m: RegExpExecArray | null;
  const atx = /^#{1,6}\s+(.+)$/gm;
  while ((m = atx.exec(source))) consider(m[1], m.index);
  const bold = /\*\*([^*\n]+)\*\*/g;
  while ((m = bold.exec(source))) consider(m[1], m.index);
  if (bestAt < 0) {
    const lines = source.split('\n');
    for (let i = 1; i < lines.length; i++) {
      if (/^\s*(-{3,}|={3,})\s*$/.test(lines[i]) && lines[i - 1].trim()) {
        consider(lines[i - 1], source.indexOf(lines[i - 1]));
        break;
      }
    }
  }
  return best ? best.replace(/[*_`#]+/g, '').trim().slice(0, 80) : source.trim().split('\n')[0].slice(0, 80);
}
