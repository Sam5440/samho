// 项目管理（增删改/配色）与项目文件夹（folders + entry 主目录）对话框。

import { useEffect, useState } from 'react';
import { CheckIcon, StarIcon, Trash2Icon, XIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { toast } from '@/components/ui/toaster';
import { Tip } from '@/components/ui/tooltip';
import { confirmAction, promptText } from '@/components/dialogs';
import { apiDelete, apiPatch, apiPost } from '@/lib/api';
import { refreshSidebar } from '@/lib/controller';
import { useStore, type Project } from '@/lib/store';
import { cn } from '@/lib/utils';

// 新项目默认色轮换（属于数据而非主题样式）。
const DEFAULT_COLORS = ['#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#8b5cf6', '#ec4899', '#14b8a6', '#71717a'];
let colorCursor = 0;

export function ManageProjectsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const projects = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const [name, setName] = useState('');
  const [invalid, setInvalid] = useState(false);

  useEffect(() => { if (open) refreshSidebar(); }, [open]);

  const create = async () => {
    const value = name.trim();
    if (!value) { setInvalid(true); return; }
    try {
      await apiPost('/api/projects', { name: value, color: DEFAULT_COLORS[colorCursor % DEFAULT_COLORS.length] });
      colorCursor += 1;
      setName('');
      await refreshSidebar();
      toast('已创建项目');
    } catch (error) { toast(`创建失败：${(error as Error).message}`, 'error'); }
  };

  const recolor = async (project: Project, color: string) => {
    try {
      await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { color });
      await refreshSidebar();
    } catch (error) { toast(`颜色保存失败：${(error as Error).message}`, 'error'); }
  };

  const rename = async (project: Project) => {
    const next = await promptText({ title: '重命名项目', label: '项目名称', value: project.name, maxLength: 64 });
    if (!next) return;
    try {
      await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { name: next });
      await refreshSidebar();
      toast('已重命名项目');
    } catch (error) { toast(`重命名失败：${(error as Error).message}`, 'error'); }
  };

  const remove = async (project: Project) => {
    const count = sessions.filter((s) => s.project_id === project.id).length;
    const ok = await confirmAction({
      title: '删除项目',
      description: `项目“${project.name}”将被删除，${count} 个会话回到未分类。会话本身与执行轨迹不受影响。`,
      confirmText: '删除项目',
      destructive: true,
    });
    if (!ok) return;
    try {
      await apiDelete(`/api/projects/${encodeURIComponent(project.id)}`);
      await refreshSidebar();
      toast('已删除项目，相关会话回到未分类');
    } catch (error) { toast(`删除失败：${(error as Error).message}`, 'error'); }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>管理项目</DialogTitle>
          <DialogDescription>项目用来给会话分类。删除项目不会删除会话。</DialogDescription>
        </DialogHeader>
        <div className="grid max-h-[50vh] gap-1.5 overflow-y-auto">
          {!projects.length && <p className="rounded-lg border border-dashed p-4 text-center text-sm text-muted-foreground">还没有项目。创建一个，然后把会话移动进去分类管理。</p>}
          {projects.map((project) => {
            const count = sessions.filter((s) => s.project_id === project.id).length;
            return (
              <div key={project.id} className="flex items-center gap-3 rounded-lg border px-3 py-2">
                <span className="size-2.5 shrink-0 rounded-full" style={{ background: project.color || 'var(--muted-foreground)' }} />
                <button type="button" title="点击重命名" onClick={() => rename(project)} className="min-w-0 flex-1 truncate text-left text-sm font-medium hover:underline">
                  {project.name}
                </button>
                <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{count} 会话</span>
                <div className="flex shrink-0 gap-1">
                  {DEFAULT_COLORS.map((color) => {
                    const active = (project.color || '').toLowerCase() === color;
                    return (
                      <button
                        key={color}
                        type="button"
                        aria-label={`把「${project.name}」设为该颜色`}
                        aria-pressed={active}
                        onClick={() => recolor(project, color)}
                        className={cn('grid size-4 place-items-center rounded-full ring-offset-1 ring-offset-background transition-shadow hover:ring-2 hover:ring-ring', active && 'ring-2 ring-foreground')}
                        style={{ background: color }}
                      >
                        {active && <CheckIcon className="size-2.5 text-white" />}
                      </button>
                    );
                  })}
                </div>
                <Tip text="删除项目">
                  <Button variant="ghost" size="icon-xs" aria-label="删除项目" className="text-muted-foreground hover:text-destructive" onClick={() => remove(project)}>
                    <Trash2Icon />
                  </Button>
                </Tip>
              </div>
            );
          })}
        </div>
        <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); create(); }}>
          <Input
            value={name}
            maxLength={64}
            placeholder="新项目名称…"
            aria-label="项目名称"
            aria-invalid={invalid || undefined}
            onChange={(e) => { setName(e.target.value); setInvalid(false); }}
          />
          <Button type="submit">创建项目</Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function FoldersDialog({ project, open, onOpenChange }: { project: Project; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [value, setValue] = useState('');
  const [invalid, setInvalid] = useState(false);
  const folders = Array.isArray(project.folders) ? project.folders : [];

  // entry 语义：undefined=不动；null=清除；字符串=设置。
  const persist = async (nextFolders?: string[], nextEntry?: string | null) => {
    const body: Record<string, unknown> = {};
    if (nextFolders) body.folders = nextFolders;
    if (nextEntry !== undefined) body.entry = nextEntry;
    try {
      await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, body);
      await refreshSidebar();
      return true;
    } catch (error) {
      toast(`保存失败：${(error as Error).message}`, 'error');
      return false;
    }
  };

  const add = async () => {
    const path = value.trim();
    if (!path) { setInvalid(true); return; }
    if (await persist([...folders, path], project.entry ?? null)) setValue('');
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>项目文件夹 · {project.name}</DialogTitle>
          <DialogDescription>项目可关联 workspace 内多个文件夹作为常驻上下文；主目录是新会话的默认起始目录。</DialogDescription>
        </DialogHeader>
        <div className="grid gap-1.5">
          {!folders.length && <p className="rounded-lg border border-dashed p-4 text-center text-sm text-muted-foreground">还没有文件夹。添加后该项目的终端/文件面板可以直达对应目录。</p>}
          {folders.map((folder) => {
            const isEntry = project.entry === folder;
            return (
              <div key={folder} className="flex items-center gap-2 rounded-lg border px-3 py-1.5">
                <code className="min-w-0 flex-1 truncate font-mono text-sm">{folder}</code>
                <Button
                  variant={isEntry ? 'secondary' : 'ghost'}
                  size="xs"
                  title="主目录是新会话终端和文件面板的默认起始目录"
                  onClick={() => persist(undefined, isEntry ? null : folder)}
                >
                  <StarIcon className={cn(isEntry && 'fill-current text-warning')} />{isEntry ? '主目录' : '设为主目录'}
                </Button>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={`移除 ${folder}`}
                  className="text-muted-foreground"
                  onClick={() => persist(folders.filter((f) => f !== folder), isEntry ? null : project.entry ?? null)}
                >
                  <XIcon />
                </Button>
              </div>
            );
          })}
        </div>
        <form className="flex gap-2" onSubmit={(e) => { e.preventDefault(); add(); }}>
          <Input
            value={value}
            maxLength={300}
            placeholder="相对路径，例如 apps/frontend 或 docs"
            aria-label="新文件夹路径"
            aria-invalid={invalid || undefined}
            spellCheck={false}
            className="font-mono"
            onChange={(e) => { setValue(e.target.value); setInvalid(false); }}
          />
          <Button type="submit">添加文件夹</Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
