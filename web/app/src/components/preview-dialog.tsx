// 沙箱 HTML 预览对话框：对话代码块（POST /api/preview 暂存）或 workspace 网页文件
// （/api/preview/fs/…，相对资源可解析，适合多文件小游戏）在 sandbox iframe 中真实运行。
// 服务端响应自带 sandbox CSP（不透明源、禁网络）；iframe sandbox 属性是第二道防线。

import { useState, useSyncExternalStore } from 'react';
import { ExternalLinkIcon, Maximize2Icon, Minimize2Icon, RotateCwIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Tip } from '@/components/ui/tooltip';
import { toast } from '@/components/ui/toaster';
import { apiPath, apiPost } from '@/lib/api';
import { cn } from '@/lib/utils';

type Preview = { title: string; src: string };

let current: Preview | null = null;
const listeners = new Set<() => void>();
const set = (next: Preview | null) => { current = next; listeners.forEach((fn) => fn()); };

const SANDBOX = 'allow-scripts allow-forms allow-modals allow-pointer-lock allow-popups allow-downloads';

const encodePath = (path: string) => path.split('/').filter(Boolean).map(encodeURIComponent).join('/');

/** 预览一段 HTML 源码（对话代码块）。 */
export async function previewHtml(html: string, title = 'HTML 预览') {
  try {
    const { id } = await apiPost<{ id: string }>('/api/preview', { html });
    set({ title, src: apiPath(`/api/preview/${encodeURIComponent(id)}`) });
  } catch (error) {
    toast(`预览失败：${(error as Error).message}`, 'error');
  }
}

/** 预览 workspace 内的网页文件（相对脚本/样式/图片按文件目录解析）。 */
export function previewWorkspaceFile(path: string) {
  set({ title: path, src: apiPath(`/api/preview/fs/${encodePath(path)}`) });
}

export function PreviewHost() {
  const preview = useSyncExternalStore((fn) => { listeners.add(fn); return () => listeners.delete(fn); }, () => current);
  const [full, setFull] = useState(false);
  const [nonce, setNonce] = useState(0);
  return (
    <Dialog open={Boolean(preview)} onOpenChange={(open) => { if (!open) set(null); }}>
      <DialogContent
        showCloseButton
        className={cn(
          'flex flex-col gap-2 p-3 sm:max-w-none',
          full ? 'h-[calc(100vh-1rem)] w-[calc(100vw-1rem)] max-h-none' : 'h-[min(85vh,52rem)] w-[min(72rem,calc(100vw-2rem))]',
        )}
      >
        <DialogHeader className="flex-row items-center gap-1 pr-8">
          <DialogTitle className="min-w-0 flex-1 truncate font-mono text-sm font-medium">{preview?.title}</DialogTitle>
          <DialogDescription className="sr-only">沙箱预览：脚本可运行，但无法访问工作台与网络接口。</DialogDescription>
          <Tip text="重新加载"><Button variant="ghost" size="icon-xs" aria-label="重新加载" onClick={() => setNonce((n) => n + 1)}><RotateCwIcon /></Button></Tip>
          <Tip text={full ? '退出全屏' : '全屏'}>
            <Button variant="ghost" size="icon-xs" aria-label={full ? '退出全屏' : '全屏'} onClick={() => setFull(!full)}>
              {full ? <Minimize2Icon /> : <Maximize2Icon />}
            </Button>
          </Tip>
          <Tip text="在新标签页打开">
            <Button variant="ghost" size="icon-xs" aria-label="在新标签页打开" onClick={() => preview && window.open(preview.src, '_blank', 'noopener')}><ExternalLinkIcon /></Button>
          </Tip>
        </DialogHeader>
        {preview && (
          <iframe
            key={`${preview.src}#${nonce}`}
            title={`预览：${preview.title}`}
            src={preview.src}
            sandbox={SANDBOX}
            allow="fullscreen; gamepad; autoplay"
            className="min-h-0 w-full flex-1 rounded-md border bg-white"
          />
        )}
        <p className="text-[11px] text-muted-foreground">沙箱运行：脚本可执行，但与工作台隔离且禁止网络请求。</p>
      </DialogContent>
    </Dialog>
  );
}
