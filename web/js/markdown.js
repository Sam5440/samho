// 轻量安全 markdown 渲染器：先全量 HTML 转义，再生成白名单标签。
// 不支持原始 HTML 透传——用户/模型文本中的一切标签按字面显示。

const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const escapeHtml = (text) => String(text).replace(/[&<>"']/g, (ch) => ESC[ch]);

function safeUrl(raw) {
  const url = String(raw).trim();
  return /^(https?:\/\/|mailto:)/i.test(url) ? url : null;
}

function inline(text) {
  let out = escapeHtml(text);
  const codes = [];
  out = out.replace(/`([^`\n]+)`/g, (_, code) => {
    codes.push(`<code class="md-code-inline">${code}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  out = out.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_, label, url) => {
    const href = safeUrl(url);
    return href ? `<a href="${href}" target="_blank" rel="noopener noreferrer">${label}</a>` : label;
  });
  out = out.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');
  out = out.replace(/(^|[\s(（])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  out = out.replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  out = out.replace(/(^|[^&])\u0000(\d+)\u0000/g, (m, p, i) => codes[Number(i)] ?? m);
  // 兜底：还原未被消费的占位符
  out = out.replace(/\u0000(\d+)\u0000/g, (_, i) => codes[Number(i)] ?? '');
  return out;
}

function renderBlocks(lines) {
  const html = [];
  let i = 0;
  const peek = () => lines[i];
  const isBlank = (line) => !line || !line.trim();

  while (i < lines.length) {
    const line = peek();
    if (isBlank(line)) { i++; continue; }

    // fenced code block
    const fence = line.match(/^```([\w+-]*)\s*$/);
    if (fence) {
      i++;
      const body = [];
      while (i < lines.length && !/^```\s*$/.test(lines[i])) { body.push(lines[i]); i++; }
      i++; // skip closing fence (or EOF)
      const lang = fence[1] ? `<span class="md-code-lang">${escapeHtml(fence[1])}</span>` : '';
      html.push(`<div class="md-code-block">${lang}<pre><code>${escapeHtml(body.join('\n'))}</code></pre></div>`);
      continue;
    }

    // heading
    const heading = line.match(/^(#{1,6})\s+(.*)$/);
    if (heading) {
      const level = heading[1].length;
      html.push(`<h${level} class="md-h">${inline(heading[2])}</h${level}>`);
      i++;
      continue;
    }

    // horizontal rule
    if (/^\s{0,3}(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { html.push('<hr class="md-hr">'); i++; continue; }

    // blockquote
    if (/^\s*>\s?/.test(line)) {
      const body = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) { body.push(lines[i].replace(/^\s*>\s?/, '')); i++; }
      html.push(`<blockquote class="md-quote">${renderBlocks(body.join('\n').split('\n'))}</blockquote>`);
      continue;
    }

    // table: | a | b |  /  | --- | --- |
    if (/^\s*\|.*\|\s*$/.test(line) && i + 1 < lines.length && /^\s*\|[\s:|-]+\|\s*$/.test(lines[i + 1])) {
      const cells = (row) => row.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
      const head = cells(line);
      i += 2;
      const rows = [];
      while (i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i])) { rows.push(cells(lines[i])); i++; }
      html.push('<div class="md-table-wrap"><table class="md-table"><thead><tr>'
        + head.map((c) => `<th>${inline(c)}</th>`).join('')
        + '</tr></thead><tbody>'
        + rows.map((row) => `<tr>${row.map((c) => `<td>${inline(c)}</td>`).join('')}</tr>`).join('')
        + '</tbody></table></div>');
      continue;
    }

    // lists
    const bullet = line.match(/^\s*[-*+]\s+/);
    const ordered = line.match(/^\s*(\d+)[.)]\s+/);
    if (bullet || ordered) {
      const tag = bullet ? 'ul' : 'ol';
      const items = [];
      while (i < lines.length) {
        const current = lines[i];
        const m = bullet ? current.match(/^\s*[-*+]\s+(.*)$/) : current.match(/^\s*\d+[.)]\s+(.*)$/);
        if (m) { items.push(m[1]); i++; }
        else if (!isBlank(current) && /^\s{2,}\S/.test(current) && items.length) { items[items.length - 1] += '\n' + current.trim(); i++; }
        else break;
      }
      html.push(`<${tag} class="md-list">${items.map((item) => `<li>${inline(item).replace(/\n/g, '<br>')}</li>`).join('')}</${tag}>`);
      continue;
    }

    // paragraph
    const para = [];
    while (i < lines.length && !isBlank(lines[i])
      && !/^```/.test(lines[i]) && !/^(#{1,6})\s/.test(lines[i])
      && !/^\s*>\s?/.test(lines[i]) && !/^\s*[-*+]\s+/.test(lines[i])
      && !/^\s*\d+[.)]\s+/.test(lines[i]) && !/^\s*\|.*\|\s*$/.test(lines[i])) {
      para.push(lines[i]);
      i++;
    }
    if (para.length) html.push(`<p class="md-p">${inline(para.join('\n')).replace(/\n/g, '<br>')}</p>`);
    else i++;
  }
  return html.join('');
}

export function renderMarkdown(text) {
  const source = String(text ?? '');
  return renderBlocks(source.replace(/\r\n?/g, '\n').split('\n'));
}

// 从推理文本提取“当前正在想什么”的标题（OpenCode reasoningHeading 的移植）：
// 依次尝试 HTML 标题 / ATX 标题 / setext 标题 / 粗体行，取位置最靠后者。
export function reasoningHeading(text) {
  const source = String(text ?? '');
  if (!source.trim()) return '';
  let best = '';
  let bestAt = -1;
  const consider = (match, offset) => {
    if (match && offset >= bestAt) { best = match.trim(); bestAt = offset; }
  };
  let m;
  const htmlTag = /<h[1-6][^>]*>([^<]+)<\/h[1-6]>/gi;
  while ((m = htmlTag.exec(source))) consider(m[1], m.index);
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
