// 文件管理面板：workspace 目录树（惰性加载）+ 文本编辑保存 + 图片预览
// + 新建文件/目录、重命名、删除。所有路径由服务端限制在 workspace 根内。

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ChevronRightIcon, DownloadIcon, FileIcon, FileImageIcon, FilePlusIcon, FolderIcon, FolderOpenIcon, FolderPlusIcon,
  MoreHorizontalIcon, PencilIcon, RefreshCwIcon, SaveIcon, Trash2Icon, XIcon, LinkIcon, EyeIcon, CodeIcon, PlayIcon,
} from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Tip } from '@/components/ui/tooltip';
import { toast } from '@/components/ui/toaster';
import { confirmAction, promptText } from '@/components/dialogs';
import { Markdown } from '@/components/markdown';
import { previewWorkspaceFile } from '@/components/preview-dialog';
import { EmptyHint, PanelToolbar } from '@/components/panels/shared';
import { api, apiDelete, apiPath, apiPost, apiPut } from '@/lib/api';
import { cn } from '@/lib/utils';

const IMAGE_EXT = /\.(png|jpe?g|gif|svg|webp|ico|bmp)$/i;
const MARKDOWN_EXT = /\.(md|markdown|mdx)$/i;
const HTML_EXT = /\.(html?|svg)$/i;
type Entry = { name: string; kind: 'dir' | 'file' | 'link' };
type Preview =
  | { kind: 'image'; path: string }
  | { kind: 'binary'; path: string }
  | { kind: 'text'; path: string; content: string; saved: string; truncated: boolean };

const join = (dir: string, name: string) => (dir ? `${dir}/${name}` : name);
const parentOf = (path: string) => { const i = path.lastIndexOf('/'); return i === -1 ? '' : path.slice(0, i); };
const baseName = (path: string) => path.slice(path.lastIndexOf('/') + 1) || path || 'workspace';

export function FilesPanel({ active }: { active: boolean }) {
  const [cwd, setCwd] = useState('');
  const [cache, setCache] = useState<Record<string, Entry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  // Markdown 文件默认渲染视图，可切换回源码编辑。
  const [rendered, setRendered] = useState(true);
  const stateRef = useRef({ cwd, expanded });
  stateRef.current = { cwd, expanded };

  const listDir = useCallback(async (path: string) => {
    const data = await api(`/api/fs?path=${encodeURIComponent(path)}`);
    const entries: Entry[] = Array.isArray(data.entries) ? data.entries : [];
    setCache((prev) => ({ ...prev, [path]: entries }));
    return entries;
  }, []);

  // 刷新 = 失效缓存后重列根目录、当前目录与全部展开目录（切工作区后路径空间已变）。
  const refresh = useCallback(async () => {
    const { cwd: dir, expanded: open } = stateRef.current;
    setCache({});
    const targets = [...new Set(['', dir, ...open])];
    await Promise.all(targets.map((p) => listDir(p).catch(() => {})));
  }, [listDir]);

  useEffect(() => { if (active) refresh(); }, [active, refresh]);
  useEffect(() => {
    const onWs = () => { setSelected(''); setPreview(null); setCwd(''); setExpanded(new Set()); if (stateRef.current) refresh(); };
    document.addEventListener('workspace-changed', onWs);
    return () => document.removeEventListener('workspace-changed', onWs);
  }, [refresh]);

  // 对话中的 workspace 相对链接 / 工具卡「在文件面板打开」。
  const openFileRef = useRef<(path: string) => void>(() => {});
  useEffect(() => {
    const onOpen = (event: Event) => {
      const path = String((event as CustomEvent<string>).detail || '').replace(/^\/+/, '');
      if (path) openFileRef.current(path);
    };
    document.addEventListener('open-workspace-file', onOpen);
    return () => document.removeEventListener('open-workspace-file', onOpen);
  }, []);

  const goto = async (path: string) => {
    setCwd(path);
    try { await listDir(path); } catch (error) { toast(`读取目录失败：${(error as Error).message}`, 'error'); }
  };

  const toggleDir = async (path: string) => {
    if (expanded.has(path)) {
      setExpanded((prev) => { const next = new Set(prev); next.delete(path); return next; });
      return;
    }
    try {
      await listDir(path);
      setExpanded((prev) => new Set(prev).add(path));
    } catch (error) { toast(`读取目录失败：${(error as Error).message}`, 'error'); }
  };

  const openFile = async (path: string) => {
    setSelected(path);
    setRendered(true);
    if (IMAGE_EXT.test(path)) { setPreview({ kind: 'image', path }); return; }
    try {
      const data = await api(`/api/file?path=${encodeURIComponent(path)}`);
      if (data.binary) { setPreview({ kind: 'binary', path }); return; }
      const content = data.content ?? '';
      setPreview({ kind: 'text', path, content, saved: content, truncated: Boolean(data.truncated) });
    } catch (error) { toast(`读取失败：${(error as Error).message}`, 'error'); }
  };

  openFileRef.current = (path: string) => {
    // 展开父目录链，让树中可见所选文件。
    const parents = path.split('/').slice(0, -1).map((_, i, all) => all.slice(0, i + 1).join('/'));
    Promise.all(parents.map((p) => listDir(p).catch(() => {}))).then(() => setExpanded((prev) => new Set([...prev, ...parents])));
    openFile(path);
  };

  const save = async () => {
    if (preview?.kind !== 'text') return;
    try {
      await apiPut('/api/file', { path: preview.path, content: preview.content });
      setPreview({ ...preview, saved: preview.content });
      toast(`已保存 ${baseName(preview.path)}`);
    } catch (error) { toast(`保存失败：${(error as Error).message}`, 'error'); }
  };

  const create = async (kind: 'file' | 'dir') => {
    const raw = await promptText({
      title: kind === 'dir' ? '新建目录' : '新建文件',
      description: '相对 workspace 根；父目录不存在时自动创建。',
      label: '路径',
      value: cwd ? `${cwd}/` : '',
      placeholder: kind === 'dir' ? '目录路径，如 src/lib' : '文件路径，如 notes/todo.md',
      confirmText: '创建',
    });
    const path = raw?.replace(/^\/+/, '');
    if (!path) return;
    try {
      if (kind === 'dir') await apiPost('/api/fs/mkdir', { path });
      else await apiPut('/api/file', { path, content: '' });
      if (parentOf(path)) setExpanded((prev) => new Set(prev).add(parentOf(path)));
      await refresh();
      await listDir(parentOf(path)).catch(() => {});
      toast(`已创建 ${path}`);
    } catch (error) { toast(`创建失败：${(error as Error).message}`, 'error'); }
  };

  const rename = async (path: string) => {
    const raw = await promptText({ title: '重命名 / 移动', description: '可改为新名称或移动到其他目录（workspace 内）。', label: '新路径', value: path, confirmText: '应用' });
    const to = raw?.replace(/^\/+/, '');
    if (!to || to === path) return;
    try {
      await apiPost('/api/fs/rename', { from: path, to });
      if (selected === path) { setSelected(to); setPreview((p) => (p ? { ...p, path: to } : p)); }
      await refresh();
      toast(`已重命名为 ${to}`);
    } catch (error) { toast(`重命名失败：${(error as Error).message}`, 'error'); }
  };

  const remove = async (entry: Entry, path: string) => {
    const isDir = entry.kind === 'dir';
    const ok = await confirmAction({
      title: isDir ? '删除目录' : '删除文件',
      description: `“${path}”${isDir ? ' 及其全部内容' : ''}将被永久删除，此操作不可撤销。`,
      confirmText: '永久删除',
      destructive: true,
    });
    if (!ok) return;
    try {
      await apiDelete(`/api/fs?path=${encodeURIComponent(path)}`);
      setExpanded((prev) => { const next = new Set(prev); next.delete(path); return next; });
      if (selected === path || selected.startsWith(`${path}/`)) { setSelected(''); setPreview(null); }
      await refresh();
      toast('已删除');
    } catch (error) { toast(`删除失败：${(error as Error).message}`, 'error'); }
  };

  const renderRows = (dir: string, depth: number): React.ReactNode[] => {
    const entries = cache[dir] || [];
    return entries.flatMap((entry) => {
      const path = join(dir, entry.name);
      const isDir = entry.kind === 'dir';
      const isOpen = expanded.has(path);
      const Icon = isDir ? (isOpen ? FolderOpenIcon : FolderIcon) : IMAGE_EXT.test(path) ? FileImageIcon : FileIcon;
      const row = (
        <div
          key={path}
          role="treeitem"
          aria-expanded={isDir ? isOpen : undefined}
          aria-selected={selected === path}
          tabIndex={0}
          onClick={() => (isDir ? toggleDir(path) : openFile(path))}
          onKeyDown={(e) => { if (e.key === 'Enter') { if (isDir) toggleDir(path); else openFile(path); } }}
          className={cn(
            'group/fs flex h-7 cursor-pointer items-center gap-1.5 rounded-md pr-1 text-[13px] outline-none hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring/50',
            selected === path && 'bg-sidebar-accent text-foreground',
          )}
          style={{ paddingLeft: `${0.375 + depth * 0.875}rem` }}
        >
          <ChevronRightIcon className={cn('size-3 shrink-0 text-muted-foreground transition-transform', !isDir && 'invisible', isOpen && 'rotate-90')} />
          <Icon className={cn('size-3.5 shrink-0', isDir ? 'text-brand/80' : 'text-muted-foreground')} />
          <span className="min-w-0 flex-1 truncate">{entry.name}</span>
          {entry.kind === 'link' && <LinkIcon className="size-3 text-muted-foreground" />}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-xs" aria-label="文件操作" onClick={(e) => e.stopPropagation()}
                className="size-6 opacity-0 group-hover/fs:opacity-100 group-focus-within/fs:opacity-100 data-[state=open]:opacity-100">
                <MoreHorizontalIcon />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
              {isDir
                ? <DropdownMenuItem onSelect={async () => { await listDir(path).catch(() => {}); setExpanded((p) => new Set(p).add(path)); }}><RefreshCwIcon />刷新目录</DropdownMenuItem>
                : <DropdownMenuItem onSelect={() => openFile(path)}><FileIcon />打开文件</DropdownMenuItem>}
              {isDir && <DropdownMenuItem onSelect={() => goto(path)}><FolderOpenIcon />设为树根</DropdownMenuItem>}
              <DropdownMenuItem onSelect={() => rename(path)}><PencilIcon />重命名…</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => remove(entry, path)}><Trash2Icon />删除…</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      );
      return isDir && isOpen ? [row, ...renderRows(path, depth + 1)] : [row];
    });
  };

  const segments = cwd.split('/').filter(Boolean);
  const dirty = preview?.kind === 'text' && preview.content !== preview.saved;

  return (
    <>
      <PanelToolbar>
        <nav aria-label="当前目录" className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden font-mono text-xs">
          <button type="button" onClick={() => goto('')} className="shrink-0 rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground">workspace</button>
          {segments.map((seg, i) => {
            const path = segments.slice(0, i + 1).join('/');
            return (
              <span key={path} className="flex min-w-0 items-center gap-0.5">
                <span className="text-muted-foreground/50">/</span>
                <button type="button" onClick={() => goto(path)} className="truncate rounded px-1 text-muted-foreground hover:bg-accent hover:text-foreground">{seg}</button>
              </span>
            );
          })}
        </nav>
        <Tip text="刷新目录树"><Button variant="ghost" size="icon-xs" aria-label="刷新" onClick={refresh}><RefreshCwIcon /></Button></Tip>
        <Tip text="新建文件"><Button variant="ghost" size="icon-xs" aria-label="新文件" onClick={() => create('file')}><FilePlusIcon /></Button></Tip>
        <Tip text="新建目录"><Button variant="ghost" size="icon-xs" aria-label="新目录" onClick={() => create('dir')}><FolderPlusIcon /></Button></Tip>
      </PanelToolbar>
      <div role="tree" aria-label="目录树" className={cn('min-h-0 overflow-auto p-1', preview ? 'flex-[1_1_40%]' : 'flex-1')}>
        {!(cwd in cache) ? <p className="p-3 text-xs text-muted-foreground">正在读取…</p>
          : !cache[cwd].length ? <EmptyHint icon={FolderIcon} title="目录为空" />
            : renderRows(cwd, 0)}
      </div>
      {preview && (
        <div aria-label="文件预览" className="flex min-h-0 flex-[1_1_60%] flex-col border-t">
          <div className="flex h-9 shrink-0 items-center gap-1.5 border-b px-2">
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" title={preview.path}>{preview.path}</span>
            {preview.kind === 'text' && preview.truncated && <Badge variant="outline">已截断</Badge>}
            {preview.kind === 'binary' && <Badge variant="outline">二进制</Badge>}
            {dirty && <span className="size-1.5 rounded-full bg-warning" title="未保存" />}
            {preview.kind === 'text' && MARKDOWN_EXT.test(preview.path) && (
              <Tip text={rendered ? '编辑源码' : '渲染预览'}>
                <Button variant="ghost" size="icon-xs" aria-label={rendered ? '编辑源码' : '渲染预览'} onClick={() => setRendered(!rendered)}>
                  {rendered ? <CodeIcon /> : <EyeIcon />}
                </Button>
              </Tip>
            )}
            {HTML_EXT.test(preview.path) && (
              <Tip text="在沙箱中运行（相对资源按文件目录加载）">
                <Button variant="ghost" size="icon-xs" aria-label="运行预览" onClick={() => previewWorkspaceFile(preview.path)}><PlayIcon /></Button>
              </Tip>
            )}
            {preview.kind === 'text' && (
              <Button size="xs" disabled={!dirty} onClick={save}><SaveIcon />保存</Button>
            )}
            <Button variant="ghost" size="icon-xs" aria-label="关闭预览" onClick={() => { setPreview(null); setSelected(''); }}><XIcon /></Button>
          </div>
          {preview.kind === 'text' && MARKDOWN_EXT.test(preview.path) && rendered && (
            <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
              <Markdown text={preview.content} baseDir={parentOf(preview.path)} />
            </div>
          )}
          {preview.kind === 'text' && !(MARKDOWN_EXT.test(preview.path) && rendered) && (
            <textarea
              spellCheck={false}
              aria-label="文件内容编辑"
              value={preview.content}
              onChange={(e) => setPreview({ ...preview, content: e.target.value })}
              onKeyDown={(e) => { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); save(); } }}
              className="min-h-0 flex-1 resize-none bg-transparent p-3 font-mono text-xs leading-relaxed outline-none"
            />
          )}
          {preview.kind === 'image' && (
            <div className="grid min-h-0 flex-1 place-items-center overflow-auto bg-muted/30 p-3">
              <img className="max-h-full max-w-full object-contain" src={apiPath(`/api/file/raw?path=${encodeURIComponent(preview.path)}`)} alt={preview.path} />
            </div>
          )}
          {preview.kind === 'binary' && (
            <EmptyHint icon={FileIcon} title="二进制文件不支持内联预览">
              <a className="mt-2 inline-flex items-center gap-1 text-brand hover:underline" href={apiPath(`/api/file/raw?path=${encodeURIComponent(preview.path)}`)} download={baseName(preview.path)}>
                <DownloadIcon className="size-3.5" />下载原始文件
              </a>
            </EmptyHint>
          )}
        </div>
      )}
    </>
  );
}
