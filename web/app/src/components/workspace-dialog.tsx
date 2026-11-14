// 新建工作区：资源管理器式设备目录选择（进入/上级 + 就地新建文件夹）+ 显示名称。

import { useCallback, useEffect, useState } from 'react';
import { ArrowUpIcon, FolderIcon, FolderPlusIcon, HardDriveIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Spinner } from '@/components/ui/spinner';
import { toast } from '@/components/ui/toaster';
import { apiRoot, apiRootSend } from '@/lib/api';

type Listing = { path: string; parent: string | null; entries: { name: string; path: string }[] };

// 拼接子目录：兼容 "D:\" 与 "D:/parent" 两种基路径。
function joinPath(base: string, name: string) {
  if (!base) return name;
  if (/[\\/]$/.test(base)) return base + name;
  return base + (base.includes('\\') ? '\\' : '/') + name;
}

export function NewWorkspaceDialog({ open, onOpenChange, onCreated }: {
  open: boolean; onOpenChange: (open: boolean) => void; onCreated: (id: string) => void;
}) {
  const [listing, setListing] = useState<Listing>({ path: '', parent: null, entries: [] });
  const [loading, setLoading] = useState(false);
  const [name, setName] = useState('');
  const [folder, setFolder] = useState('');

  const load = useCallback(async (path: string) => {
    setLoading(true);
    try {
      const data = await apiRoot<Listing>(`/api/browse?path=${encodeURIComponent(path)}`);
      setListing({ path: data.path || '', parent: data.parent ?? null, entries: Array.isArray(data.entries) ? data.entries : [] });
    } catch (error) {
      toast(`无法打开目录：${(error as Error).message}`, 'error');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!open) return;
    setName('');
    setFolder('');
    load('');
  }, [open, load]);

  const mkdir = async () => {
    const value = folder.trim();
    if (!value) return;
    if (!listing.path) { toast('请先进入一个具体目录', 'error'); return; }
    try {
      const created = await apiRootSend('/api/browse/mkdir', 'POST', { path: joinPath(listing.path, value) });
      setFolder('');
      toast(`已创建文件夹：${created.path}`);
      await load(listing.path);
    } catch (error) { toast(`创建失败：${(error as Error).message}`, 'error'); }
  };

  const create = async () => {
    if (!listing.path) return;
    try {
      const created = await apiRootSend('/api/workspaces', 'POST', { path: listing.path, name: name.trim() || undefined });
      onOpenChange(false);
      toast(`已创建工作区：${created.name}`);
      onCreated(created.id);
    } catch (error) { toast(`创建失败：${(error as Error).message}`, 'error'); }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>新建工作区</DialogTitle>
          <DialogDescription>浏览设备目录并选中；也可以就地新建文件夹。运行时会为该路径创建独立运行时与数据目录。</DialogDescription>
        </DialogHeader>
        <div className="grid gap-2">
          <Label>设备目录</Label>
          <div className="flex items-center gap-2">
            <Button variant="outline" size="icon-sm" aria-label="返回上级目录" disabled={!listing.parent && !listing.path} onClick={() => load(listing.parent ?? '')}>
              <ArrowUpIcon />
            </Button>
            <code aria-live="polite" className="flex h-8 min-w-0 flex-1 items-center truncate rounded-md border bg-muted/40 px-2.5 font-mono text-xs text-muted-foreground [direction:rtl] text-left">
              {listing.path || '此设备 · 选择盘符'}
            </code>
          </div>
          <div role="listbox" aria-label="设备目录" className="h-60 overflow-y-auto rounded-lg border p-1">
            {loading && <div className="flex h-full items-center justify-center"><Spinner className="text-muted-foreground" /></div>}
            {!loading && !listing.entries.length && <p className="p-3 text-sm text-muted-foreground">此目录没有子目录。</p>}
            {!loading && listing.entries.map((entry) => (
              <button
                key={entry.path}
                type="button"
                role="option"
                aria-selected={false}
                aria-label={`进入 ${entry.name}`}
                onClick={() => load(entry.path)}
                className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none hover:bg-accent focus-visible:bg-accent"
              >
                {listing.path ? <FolderIcon className="size-4 text-muted-foreground" /> : <HardDriveIcon className="size-4 text-muted-foreground" />}
                <span className="truncate font-mono text-[13px]">{entry.name}</span>
              </button>
            ))}
          </div>
          <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); mkdir(); }}>
            <Input value={folder} maxLength={120} placeholder="在当前目录下新建文件夹" aria-label="新建文件夹名称" className="h-8" onChange={(e) => setFolder(e.target.value)} />
            <Button type="submit" variant="outline" size="sm" disabled={!folder.trim()}><FolderPlusIcon />新建</Button>
          </form>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="ws-name">显示名称</Label>
          <Input id="ws-name" value={name} maxLength={64} placeholder="留空则取目录名" onChange={(e) => setName(e.target.value)} />
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button>
          <Button disabled={!listing.path} onClick={create}>创建工作区</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
