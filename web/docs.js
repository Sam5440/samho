(() => {
  'use strict';
  const docs = Array.isArray(window.SamhoDocuments) ? window.SamhoDocuments : [];
  const byId = (id) => document.getElementById(id);
  let current = null;
  const node = (tag, text, className) => {
    const item = document.createElement(tag);
    if (text !== undefined) item.textContent = text;
    if (className) item.className = className;
    return item;
  };
  function inline(parent, text) {
    const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^\)]+\))/g;
    let offset = 0;
    for (const match of text.matchAll(pattern)) {
      parent.append(document.createTextNode(text.slice(offset, match.index)));
      const value = match[0];
      if (value.startsWith('`')) parent.append(node('code', value.slice(1, -1)));
      else if (value.startsWith('**')) parent.append(node('strong', value.slice(2, -2)));
      else {
        const link = /^\[([^\]]+)\]\(([^\)]+)\)$/.exec(value);
        const target = link[2].split('#')[0];
        const related = docs.find((doc) => doc.path === target || target.endsWith(doc.path) || doc.path.split('/').pop() === target.split('/').pop());
        if (related || /^https?:\/\//i.test(target)) {
          const a = node('a', link[1]);
          a.href = related ? '#' + encodeURIComponent(related.id) : target;
          if (!related) { a.target = '_blank'; a.rel = 'noopener noreferrer'; }
          parent.append(a);
        } else {
          const label = node('span', link[1], 'docs-file-reference');
          label.title = '本地项目文件：' + link[2];
          parent.append(label);
        }
      }
      offset = match.index + value.length;
    }
    parent.append(document.createTextNode(text.slice(offset)));
  }
  function isBlock(line) {
    return /^(#{1,6}\s|```|~~~|\s*[-*]\s|\s*\d+[.)]\s|\s*>|\s*\|)/.test(line);
  }
  function cells(line) {
    return line.trim().replace(/^\|/, '').replace(/\|$/, '').split(/(?<!\\)\|/).map((text) => text.trim().replace(/\\\|/g, '|'));
  }
  function renderMarkdown(text) {
    const container = document.createDocumentFragment();
    const lines = text.replace(/\r\n/g, '\n').split('\n');
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];
      if (!line.trim()) { i++; continue; }
      const fence = /^(```|~~~)(.*)$/.exec(line);
      if (fence) {
        const marker = fence[1];
        const language = fence[2].trim();
        const buffer = [];
        for (i++; i < lines.length && !lines[i].startsWith(marker); i++) buffer.push(lines[i]);
        i++;
        container.append(node('div', language === 'mermaid' ? '架构图源文本' : '代码示例' + (language ? ' / ' + language : ''), 'docs-code-caption'));
        const pre = node('pre'); pre.append(node('code', buffer.join('\n'))); container.append(pre); continue;
      }
      const heading = /^(#{1,6})\s+(.+)$/.exec(line);
      if (heading) { const h = node('h' + heading[1].length); inline(h, heading[2]); container.append(h); i++; continue; }
      if (/^\s*\|/.test(line) && /^\s*\|?[\s:|-]+\|\s*$/.test(lines[i + 1] || '')) {
        const wrap = node('div', undefined, 'docs-table-wrap'); const table = node('table'); const thead = node('thead'); const header = node('tr');
        cells(line).forEach((cell) => { const th = node('th'); inline(th, cell); header.append(th); }); thead.append(header); table.append(thead);
        const tbody = node('tbody'); i += 2;
        while (i < lines.length && /^\s*\|/.test(lines[i])) {
          const tr = node('tr'); cells(lines[i++]).forEach((cell) => { const td = node('td'); inline(td, cell); tr.append(td); }); tbody.append(tr);
        }
        table.append(tbody); wrap.append(table); container.append(wrap); continue;
      }
      if (/^\s*[-*]\s/.test(line) || /^\s*\d+[.)]\s/.test(line)) {
        const ordered = /^\s*\d+[.)]\s/.test(line); const list = node(ordered ? 'ol' : 'ul');
        const regex = ordered ? /^\s*\d+[.)]\s/ : /^\s*[-*]\s/;
        while (i < lines.length && regex.test(lines[i])) { const li = node('li'); inline(li, lines[i++].replace(regex, '')); list.append(li); }
        container.append(list); continue;
      }
      if (/^\s*>/.test(line)) { const quote = node('blockquote'); inline(quote, line.replace(/^\s*>\s?/, '')); container.append(quote); i++; continue; }
      const paragraph = [line];
      for (i++; i < lines.length && lines[i].trim() && !isBlock(lines[i]); i++) paragraph.push(lines[i]);
      const p = node('p'); inline(p, paragraph.join('\n')); container.append(p);
    }
    return container;
  }
  function renderList() {
    const query = byId('docs-search').value.trim().toLowerCase();
    const filtered = docs.filter((doc) => (doc.title + doc.category + doc.body).toLowerCase().includes(query));
    const list = byId('docs-list'); list.replaceChildren();
    let category = '';
    filtered.forEach((doc) => {
      if (category !== doc.category) { category = doc.category; list.append(node('h2', category, 'docs-group')); }
      const button = node('button', doc.title, 'docs-item' + (current && current.id === doc.id ? ' active' : ''));
      button.type = 'button'; button.setAttribute('aria-current', current && current.id === doc.id ? 'page' : 'false');
      button.addEventListener('click', () => { location.hash = encodeURIComponent(doc.id); }); list.append(button);
    });
    byId('docs-count').textContent = filtered.length + ' / ' + docs.length + ' 份中文文档';
    if (!filtered.length) list.append(node('p', '没有找到匹配内容，请换一个关键词。'));
  }
  function select() {
    let id = location.hash.slice(1);
    try { id = decodeURIComponent(id); } catch (_) { id = ''; }
    current = docs.find((doc) => doc.id === id) || docs[0];
    if (!current) { byId('docs-error').hidden = false; byId('docs-error').textContent = '资料包未找到，请运行 python scripts/build-chinese-docs.py 后刷新页面。'; return; }
    byId('docs-title').textContent = current.title;
    byId('docs-category').textContent = current.category;
    byId('docs-path').textContent = '原始文件：' + current.path;
    byId('docs-body').replaceChildren(renderMarkdown(current.body));
    document.title = current.title + ' · Samho 中文文档'; renderList();
  }
  byId('docs-search').addEventListener('input', renderList);
  byId('docs-download').addEventListener('click', () => {
    if (!current) return;
    const url = URL.createObjectURL(new Blob(['\ufeff' + current.body], { type: 'text/markdown;charset=utf-8' }));
    const a = node('a'); a.href = url; a.download = current.title + '.md'; document.body.append(a); a.click(); a.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  window.addEventListener('hashchange', select); select();
  window.SamhoDocs = { renderMarkdown };
})();
