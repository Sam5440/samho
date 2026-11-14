// Markdown 正文组件：渲染 renderMarkdown 的净化输出，并以事件委托处理
// 代码块「复制 / 预览」按钮与 workspace 相对链接（交给文件面板打开）。

import { useMemo, type MouseEvent } from 'react';
import { toast } from '@/components/ui/toaster';
import { previewHtml } from '@/components/preview-dialog';
import { renderMarkdown } from '@/lib/markdown';
import { activatePanel } from '@/lib/controller';
import { setState } from '@/lib/store';
import { cn } from '@/lib/utils';

export function openWorkspaceFile(path: string) {
  activatePanel('files');
  setState({ inspectorOpen: true });
  // 文件面板始终挂载，监听该事件打开指定文件。
  document.dispatchEvent(new CustomEvent('open-workspace-file', { detail: path }));
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast('已复制');
  } catch {
    toast('复制失败：浏览器拒绝访问剪贴板（局域网 http 访问时常见）', 'error');
  }
}

export function Markdown({ text, className, baseDir }: { text: string; className?: string; baseDir?: string }) {
  // renderMarkdown 输出经 DOMPurify 净化（无脚本/事件属性/内联样式）。
  const html = useMemo(() => renderMarkdown(text, baseDir), [text, baseDir]);

  const onClick = (event: MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const action = target.closest<HTMLElement>('[data-md-action]');
    if (action) {
      const block = action.closest<HTMLElement>('.md-code-block');
      const code = block?.querySelector('code')?.textContent ?? '';
      if (action.dataset.mdAction === 'copy') copyText(code);
      if (action.dataset.mdAction === 'preview') previewHtml(code, `${block?.dataset.lang || 'html'} 代码块`);
      return;
    }
    const link = target.closest<HTMLAnchorElement>('a[data-md-file]');
    if (link) {
      event.preventDefault();
      openWorkspaceFile(link.dataset.mdFile || '');
    }
  };

  return <div className={cn('md', className)} onClick={onClick} dangerouslySetInnerHTML={{ __html: html }} />;
}
