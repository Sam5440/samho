// 工作区侧栏：工作区切换（下拉）+ 当前工作区的项目/文件夹/会话树。
// 新建/重命名/摘除工作区走全局 /api/workspaces（不受当前前缀影响）。

import { useMemo, useState } from 'react';
import {
  ArchiveIcon, BoxesIcon, CheckIcon, ChevronRightIcon, ChevronsUpDownIcon, FolderIcon, FolderPlusIcon,
  GitForkIcon, GlobeIcon, MoreHorizontalIcon, PencilIcon, PlusIcon, RefreshCwIcon, SearchIcon,
  SquarePenIcon, TerminalSquareIcon, Trash2Icon, UnplugIcon, FolderInputIcon, ArchiveRestoreIcon, StarIcon,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Spinner } from '@/components/ui/spinner';
import { Tip } from '@/components/ui/tooltip';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator,
  DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { toast } from '@/components/ui/toaster';
import { confirmAction, promptText } from '@/components/dialogs';
import { NewWorkspaceDialog } from '@/components/workspace-dialog';
import { ManageProjectsDialog, FoldersDialog } from '@/components/projects-dialogs';
import { apiDelete, apiPatch, apiRootSend } from '@/lib/api';
import {
  forkSession, loadSession, newSession, openAttachedBrowser, openAttachedTerminal, refreshSessions, refreshSidebar,
  reloadWorkspaces, switchWorkspace,
} from '@/lib/controller';
import { relativeTime, useTick } from '@/lib/hooks';
import { bridge } from '@/lib/bridge';
import { getState, sessionTitle, setState, useStore, type Project, type Session, type WorkspaceRecord } from '@/lib/store';
import { cn } from '@/lib/utils';

// Windows 规范化路径带 \\?\ 前缀，展示时去掉。
const displayPath = (path?: string) => String(path || '/').replace(/^\\\\\?\\/, '');

/* ---------- 工作区切换 ---------- */
function WorkspaceSwitcher() {
  const workspaces = useStore((s) => s.workspaces);
  const currentId = useStore((s) => s.currentWorkspaceId);
  const [creating, setCreating] = useState(false);
  const current = workspaces.find((w) => w.id === currentId);

  const rename = async (record: WorkspaceRecord) => {
    const name = await promptText({ title: '重命名工作区', label: '工作区名称', value: record.name, maxLength: 64 });
    if (!name) return;
    try {
      await apiRootSend(`/api/workspaces/${encodeURIComponent(record.id)}`, 'PATCH', { name });
      toast('已重命名工作区');
      reloadWorkspaces();
    } catch (error) { toast(`重命名失败：${(error as Error).message}`, 'error'); }
  };

  const remove = async (record: WorkspaceRecord) => {
    const ok = await confirmAction({
      title: '摘除工作区',
      description: `“${record.name}”将从本服务摘除并停止其运行时；磁盘上的会话与数据目录保留，重启后不再自动挂载。`,
      confirmText: '摘除',
      destructive: true,
    });
    if (!ok) return;
    try {
      await apiRootSend(`/api/workspaces/${encodeURIComponent(record.id)}`, 'DELETE');
      toast('已摘除工作区');
      if (record.id === currentId) switchWorkspace('default');
      else reloadWorkspaces();
    } catch (error) { toast(`摘除失败：${(error as Error).message}`, 'error'); }
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="切换工作区"
            className="flex w-full items-center gap-2.5 rounded-lg p-2 text-left outline-none hover:bg-sidebar-accent focus-visible:ring-[3px] focus-visible:ring-ring/50 data-[state=open]:bg-sidebar-accent"
          >
            <span className="grid size-8 shrink-0 place-items-center rounded-md border bg-background text-muted-foreground">
              <BoxesIcon className="size-4" />
            </span>
            <span className="grid min-w-0 flex-1 leading-tight">
              <span className="truncate text-sm font-semibold">{current?.name || currentId}</span>
              <span className="truncate font-mono text-[11px] text-muted-foreground" title={displayPath(current?.path)}>{displayPath(current?.path)}</span>
            </span>
            <ChevronsUpDownIcon className="size-4 shrink-0 text-muted-foreground" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-[min(18rem,var(--radix-dropdown-menu-trigger-width))] min-w-64">
          <DropdownMenuLabel>工作区</DropdownMenuLabel>
          {workspaces.length === 0 && <div className="px-2 py-1.5 text-xs text-muted-foreground">正在读取工作区…</div>}
          {workspaces.map((record) => (
            <DropdownMenuSub key={record.id}>
              <DropdownMenuSubTrigger className="gap-2">
                <span className="grid min-w-0 flex-1">
                  <span className="flex items-center gap-1.5 truncate">
                    {record.name || record.id}
                    {record.managed && <Badge variant="outline" className="px-1 py-0 text-[10px]">托管</Badge>}
                  </span>
                  <span className="truncate font-mono text-[11px] text-muted-foreground">{displayPath(record.path)}</span>
                </span>
                {record.id === currentId && <CheckIcon className="size-4 text-brand" />}
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent>
                <DropdownMenuItem onSelect={() => switchWorkspace(record.id)}><CheckIcon />切换到此工作区</DropdownMenuItem>
                <DropdownMenuItem onSelect={() => rename(record)}><PencilIcon />重命名…</DropdownMenuItem>
                {record.managed && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem variant="destructive" onSelect={() => remove(record)}><UnplugIcon />摘除工作区…</DropdownMenuItem>
                  </>
                )}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          ))}
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => setCreating(true)}><PlusIcon />新建工作区…</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <NewWorkspaceDialog open={creating} onOpenChange={setCreating} onCreated={(id) => switchWorkspace(id)} />
    </>
  );
}

/* ---------- 会话树工具 ---------- */
function treeRows(sessions: Session[]) {
  const byId = new Map(sessions.map((s) => [s.id, s]));
  const children = new Map<string, Session[]>();
  const roots: Session[] = [];
  for (const s of sessions) {
    if (s.parent_id && byId.has(s.parent_id)) {
      if (!children.has(s.parent_id)) children.set(s.parent_id, []);
      children.get(s.parent_id)!.push(s);
    } else roots.push(s);
  }
  const rows: { session: Session; depth: number }[] = [];
  const walk = (list: Session[], depth: number) => {
    for (const s of list) {
      rows.push({ session: s, depth });
      const kids = children.get(s.id);
      if (kids?.length) walk(kids, Math.min(depth + 1, 3));
    }
  };
  walk(roots, 0);
  return rows;
}

function sortSessions(sessions: Session[], running: Record<string, { startedAt: number }>) {
  return [...sessions].sort((a, b) => {
    const runA = running[a.id] ? 1 : 0;
    const runB = running[b.id] ? 1 : 0;
    if (runA !== runB) return runB - runA;
    return Number(b.updated_at || 0) - Number(a.updated_at || 0);
  });
}

// 今天 / 昨天 / 更早 的本地日历日分组。
function groupSessions(sessions: Session[]) {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const yesterday = today - 86_400_000;
  const groups = [
    { id: 'today', title: '今天', sessions: [] as Session[] },
    { id: 'yesterday', title: '昨天', sessions: [] as Session[] },
    { id: 'older', title: '更早', sessions: [] as Session[] },
  ];
  for (const session of sessions) {
    const stamp = Number(session.updated_at || session.created_at || 0);
    if (stamp >= today) groups[0].sessions.push(session);
    else if (stamp >= yesterday) groups[1].sessions.push(session);
    else groups[2].sessions.push(session);
  }
  return groups.filter((g) => g.sessions.length);
}

/* ---------- 会话行 ---------- */
function StatusDot({ session, running }: { session: Session; running: boolean }) {
  if (running) return <Spinner className="size-3 text-brand" />;
  const status = String(session.status || 'ready');
  if (status === 'failed' || status === 'error') return <span title="上次执行失败" className="size-1.5 shrink-0 rounded-full bg-destructive" />;
  if (status === 'interrupted') return <span title="上次被中断" className="size-1.5 shrink-0 rounded-full bg-warning" />;
  return <span className="size-1.5 shrink-0 rounded-full bg-muted-foreground/30" />;
}

function SessionRow({ session, depth, attachOpen, onToggleAttach }: {
  session: Session; depth: number; attachOpen: boolean; onToggleAttach: () => void;
}) {
  const selected = useStore((s) => s.currentSessionId === session.id);
  const running = useStore((s) => Boolean(s.runningSessions[session.id]));
  const projects = useStore((s) => s.projects);
  useStore((s) => s.attachVersion);
  const title = sessionTitle(session);
  const terms = bridge.terminal?.counts(session.id) || 0;
  const tabs = bridge.browser?.counts(session.id) || 0;

  const rename = async () => {
    const next = await promptText({ title: '重命名会话', label: '会话标题', value: title, maxLength: 200 });
    if (!next) return;
    try {
      await apiPatch(`/api/sessions/${encodeURIComponent(session.id)}`, { title: next });
      await refreshSessions();
      toast('已重命名会话');
    } catch (error) { toast(`重命名失败：${(error as Error).message}`, 'error'); }
  };

  const setArchived = async (archived: boolean) => {
    try {
      await apiPatch(`/api/sessions/${encodeURIComponent(session.id)}`, { archived });
      await refreshSessions();
      toast(archived ? '已归档，可在项目的「归档」分组中找到' : '已取消归档');
    } catch (error) { toast(`归档操作失败：${(error as Error).message}`, 'error'); }
  };

  const move = async (projectId: string | null) => {
    try {
      await apiPatch(`/api/sessions/${encodeURIComponent(session.id)}`, { project_id: projectId });
      await refreshSessions();
      toast(projectId ? '已移动到项目' : '已移出项目');
    } catch (error) { toast(`移动失败：${(error as Error).message}`, 'error'); }
  };

  const remove = async () => {
    const ok = await confirmAction({
      title: '删除会话',
      description: `“${title}”及其所有分支将被永久删除，执行轨迹保留在事件日志中。`,
      confirmText: '永久删除',
      destructive: true,
    });
    if (!ok) return;
    try {
      await apiDelete(`/api/sessions/${encodeURIComponent(session.id)}`);
      if (getState().currentSessionId === session.id) newSession();
      await refreshSessions();
      toast('会话已删除');
    } catch (error) { toast(`删除失败：${(error as Error).message}`, 'error'); }
  };

  return (
    <div>
      <div
        className={cn(
          'group/row relative flex items-center rounded-md transition-colors hover:bg-sidebar-accent',
          selected && 'bg-sidebar-accent',
        )}
        style={depth ? { paddingLeft: `${depth * 0.75}rem` } : undefined}
      >
        {selected && <span className="absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-brand" aria-hidden="true" />}
        <button
          type="button"
          aria-current={selected ? 'page' : undefined}
          onClick={() => loadSession(session.id)}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-md py-1.5 pr-1 pl-2.5 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
        >
          {depth > 0 && <GitForkIcon className="size-3 shrink-0 text-muted-foreground" />}
          <StatusDot session={session} running={running} />
          <span className="grid min-w-0 flex-1">
            <span className={cn('truncate text-[13px]', selected ? 'font-medium text-foreground' : 'text-sidebar-foreground/90')} title={title}>{title}</span>
            <span className="flex items-center gap-1.5 truncate text-[11px] text-muted-foreground">
              {session.last_model && <span className="truncate font-mono">{session.last_model}</span>}
              {session.parent_id && <span>分支</span>}
              <span className="shrink-0">{relativeTime(session.updated_at)}</span>
            </span>
          </span>
        </button>
        <div className={cn('flex shrink-0 items-center pr-1 opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100', (attachOpen || selected) && 'opacity-100')}>
          <Tip text="附属终端 / 浏览器">
            <Button variant="ghost" size="icon-xs" aria-label="附属面板" aria-expanded={attachOpen} onClick={onToggleAttach}
              className={cn('text-muted-foreground', (terms || tabs) && 'text-brand')}>
              <ChevronRightIcon className={cn('transition-transform', attachOpen && 'rotate-90')} />
            </Button>
          </Tip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-xs" aria-label="会话操作" className="text-muted-foreground"><MoreHorizontalIcon /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onSelect={() => loadSession(session.id)}><SquarePenIcon />打开会话</DropdownMenuItem>
              <DropdownMenuSub>
                <DropdownMenuSubTrigger><FolderInputIcon />移动到项目</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {projects.length === 0 && <div className="px-2 py-1.5 text-xs text-muted-foreground">还没有项目，先在「管理项目」里创建</div>}
                  {projects.map((project) => (
                    <DropdownMenuItem key={project.id} onSelect={() => move(project.id)}>
                      <span className="size-2 rounded-full" style={{ background: project.color || 'var(--muted-foreground)' }} />
                      {project.name}
                      {session.project_id === project.id && <CheckIcon className="ml-auto" />}
                    </DropdownMenuItem>
                  ))}
                  {session.project_id && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onSelect={() => move(null)}>移出项目</DropdownMenuItem>
                    </>
                  )}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={rename}><PencilIcon />重命名</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => forkSession(session.id)}><GitForkIcon />分支会话</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => setArchived(!session.archived)}>
                {session.archived ? <ArchiveRestoreIcon /> : <ArchiveIcon />}{session.archived ? '取消归档' : '归档'}
              </DropdownMenuItem>
              <DropdownMenuItem variant="destructive" onSelect={remove}><Trash2Icon />删除会话…</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      {attachOpen && (
        <div role="group" aria-label="附属面板" className="flex flex-wrap items-center gap-1 py-1 pr-1" style={{ paddingLeft: `${depth * 0.75 + 1.5}rem` }}>
          <Button variant="outline" size="xs" className="h-6 text-[11px]" title="打开/恢复该会话的附属终端" onClick={() => openAttachedTerminal(session.id)}>
            <TerminalSquareIcon />终端{terms ? ` · ${terms}` : ''}
          </Button>
          <Button variant="outline" size="xs" className="h-6 text-[11px]" title="打开/恢复该会话的附属浏览器标签" onClick={() => openAttachedBrowser(session.id)}>
            <GlobeIcon />浏览器{tabs ? ` · ${tabs}` : ''}
          </Button>
          {!terms && !tabs && <span className="text-[11px] text-muted-foreground">尚无附属面板</span>}
        </div>
      )}
    </div>
  );
}

function GroupLabel({ children, count }: { children: React.ReactNode; count?: number }) {
  return (
    <div className="flex items-center gap-1.5 px-2.5 pt-2 pb-1 text-[11px] font-medium text-muted-foreground">
      <span>{children}</span>
      {count !== undefined && <span className="ml-auto tabular-nums">{count}</span>}
    </div>
  );
}

/* ---------- 项目行 ---------- */
function ProjectNode({ project, sessions, filter, attachOpenSet, toggleAttach }: {
  project: Project; sessions: Session[]; filter: string;
  attachOpenSet: Set<string>; toggleAttach: (id: string) => void;
}) {
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [foldersOpen, setFoldersOpen] = useState(false);
  const running = useStore((s) => s.runningSessions);
  const containsCurrent = useStore((s) => sessions.some((x) => x.id === s.currentSessionId));
  // 项目展开状态由后端 open 字段兜底；点击先乐观写入 store（侧栏抽屉重挂载也保持），再持久化。
  const projectOpenState = project.open === true;
  const open = projectOpenState || Boolean(filter);

  const toggle = async () => {
    const next = !projectOpenState;
    setState((s) => ({ projects: s.projects.map((p) => (p.id === project.id ? { ...p, open: next } : p)) }));
    try { await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { open: next }); }
    catch { /* 折叠是本地优先 UI 状态，持久化失败不阻断 */ }
  };

  const rename = async () => {
    const name = await promptText({ title: '重命名项目', label: '项目名称', value: project.name, maxLength: 64 });
    if (!name) return;
    try {
      await apiPatch(`/api/projects/${encodeURIComponent(project.id)}`, { name });
      await refreshSidebar();
      toast('已重命名项目');
    } catch (error) { toast(`重命名失败：${(error as Error).message}`, 'error'); }
  };

  const filtered = sessions.filter((s) => sessionTitle(s).toLowerCase().includes(filter));
  if (filter && !filtered.length) return null;
  const active = sortSessions(filtered.filter((s) => !s.archived), running);
  const archived = filtered.filter((s) => s.archived);
  const folders = Array.isArray(project.folders) ? project.folders : [];

  return (
    <div>
      <div className={cn('group/project flex items-center rounded-md hover:bg-sidebar-accent', containsCurrent && !open && 'bg-sidebar-accent/60')}>
        <button
          type="button"
          aria-expanded={open}
          title={project.name}
          onClick={toggle}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
        >
          <ChevronRightIcon className={cn('size-3.5 shrink-0 text-muted-foreground transition-transform', open && 'rotate-90')} />
          <span className="size-2 shrink-0 rounded-full" style={{ background: project.color || 'var(--muted-foreground)' }} />
          <span className="truncate">{project.name}</span>
          <span className="ml-auto text-[11px] font-normal text-muted-foreground tabular-nums">{sessions.length}</span>
        </button>
        <div className="flex shrink-0 items-center pr-1 opacity-0 transition-opacity group-hover/project:opacity-100 group-focus-within/project:opacity-100">
          <Tip text="新任务（归属本项目）">
            <Button variant="ghost" size="icon-xs" aria-label="在该项目下新建任务" className="text-muted-foreground" onClick={() => newSession({ projectId: project.id })}>
              <SquarePenIcon />
            </Button>
          </Tip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-xs" aria-label="项目操作" className="text-muted-foreground"><MoreHorizontalIcon /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onSelect={toggle}>{projectOpenState ? '折叠项目' : '展开项目'}</DropdownMenuItem>
              <DropdownMenuItem onSelect={rename}><PencilIcon />重命名…</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => setFoldersOpen(true)}><FolderIcon />文件夹与主目录…</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => newSession({ projectId: project.id })}><PlusIcon />新任务（归属本项目）</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      {open && (
        <div className="ml-[0.9rem] border-l border-sidebar-border pl-1.5">
          {folders.map((folder) => (
            <div key={folder} className="flex items-center gap-1.5 px-2 py-1 text-xs text-muted-foreground">
              <FolderIcon className="size-3.5 shrink-0" />
              <span className="truncate font-mono" title={folder}>{folder}</span>
              {project.entry === folder && <Badge variant="outline" className="gap-0.5 px-1 py-0 text-[10px]"><StarIcon className="size-2.5" />主目录</Badge>}
            </div>
          ))}
          <button
            type="button"
            onClick={() => setFoldersOpen(true)}
            className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
          >
            <FolderPlusIcon className="size-3.5" />添加文件夹
          </button>
          {!active.length && !archived.length && (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              {filter ? '该项目下没有匹配的会话。' : '还没有会话。'}
            </p>
          )}
          {groupSessions(active).map((group) => (
            <div key={group.id}>
              <GroupLabel count={group.sessions.length}>{group.title}</GroupLabel>
              {treeRows(group.sessions).map(({ session, depth }) => (
                <SessionRow key={session.id} session={session} depth={depth}
                  attachOpen={attachOpenSet.has(session.id)} onToggleAttach={() => toggleAttach(session.id)} />
              ))}
            </div>
          ))}
          {archived.length > 0 && (
            <>
              <button
                type="button"
                aria-expanded={archiveOpen}
                onClick={() => setArchiveOpen(!archiveOpen)}
                className="flex w-full items-center gap-1.5 px-2.5 pt-2 pb-1 text-left text-[11px] font-medium text-muted-foreground hover:text-foreground"
              >
                <ChevronRightIcon className={cn('size-3 transition-transform', archiveOpen && 'rotate-90')} />
                归档
                <span className="ml-auto tabular-nums">{archived.length}</span>
              </button>
              {archiveOpen && archived.map((session) => (
                <SessionRow key={session.id} session={session} depth={0}
                  attachOpen={attachOpenSet.has(session.id)} onToggleAttach={() => toggleAttach(session.id)} />
              ))}
            </>
          )}
          <button
            type="button"
            onClick={() => newSession({ projectId: project.id })}
            className="mt-0.5 flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
          >
            <PlusIcon className="size-3.5" />新任务
          </button>
        </div>
      )}
      <FoldersDialog project={project} open={foldersOpen} onOpenChange={setFoldersOpen} />
    </div>
  );
}

/* ---------- 侧栏 ---------- */
export function Sidebar() {
  const projects = useStore((s) => s.projects);
  const sessions = useStore((s) => s.sessions);
  const running = useStore((s) => s.runningSessions);
  const [query, setQuery] = useState('');
  const [manageOpen, setManageOpen] = useState(false);
  // 附属行展开状态（默认折叠）。
  const [attachOpenSet, setAttachOpenSet] = useState<Set<string>>(() => new Set());
  useTick(60_000);
  const filter = query.trim().toLowerCase();

  const toggleAttach = (id: string) => setAttachOpenSet((prev) => {
    const next = new Set(prev);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });

  const sortedProjects = useMemo(
    () => [...projects].sort((a, b) => String(a.created_at).localeCompare(String(b.created_at))),
    [projects],
  );
  const orphans = sessions.filter((s) => s && !s.project_id);
  const orphanRows = treeRows(sortSessions(orphans.filter((s) => sessionTitle(s).toLowerCase().includes(filter)), running));

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="space-y-2 p-2 pb-1">
        <WorkspaceSwitcher />
        <Button variant="outline" className="w-full justify-start gap-2 bg-background shadow-xs" onClick={() => newSession()}>
          <SquarePenIcon />新任务
          <span className="ml-auto text-[11px] font-normal text-muted-foreground">{useStore.getState().ui.new_session_key}</span>
        </Button>
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索会话…"
            aria-label="搜索会话"
            spellCheck={false}
            className="h-8 bg-background pl-8 text-[13px]"
          />
        </div>
      </div>
      <div className="flex items-center justify-between px-3.5 pt-2 pb-1">
        <span className="text-[11px] font-medium text-muted-foreground">项目与会话</span>
        <div className="flex items-center">
          <Tip text="刷新会话列表">
            <Button id="refresh-sessions" variant="ghost" size="icon-xs" aria-label="刷新会话" className="text-muted-foreground" onClick={() => refreshSidebar()}>
              <RefreshCwIcon />
            </Button>
          </Tip>
          <Tip text="管理项目">
            <Button variant="ghost" size="icon-xs" aria-label="管理项目" className="text-muted-foreground" onClick={() => setManageOpen(true)}>
              <FolderPlusIcon />
            </Button>
          </Tip>
        </div>
      </div>
      <nav aria-label="项目与会话" className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-3">
        {!sortedProjects.length && !orphans.length && (
          <div className="mx-1 mt-2 rounded-lg border border-dashed p-4 text-center text-xs leading-relaxed text-muted-foreground">
            还没有项目或会话。<br />发送第一条消息，或
            <button type="button" className="mx-1 text-brand underline-offset-4 hover:underline" onClick={() => setManageOpen(true)}>创建项目</button>
            开始组织。
          </div>
        )}
        {sortedProjects.map((project) => (
          <ProjectNode
            key={project.id}
            project={project}
            sessions={sessions.filter((s) => s && s.project_id === project.id)}
            filter={filter}
            attachOpenSet={attachOpenSet}
            toggleAttach={toggleAttach}
          />
        ))}
        {orphanRows.length > 0 && (
          <div>
            <GroupLabel count={orphans.length}>未分类</GroupLabel>
            {orphanRows.map(({ session, depth }) => (
              <SessionRow key={session.id} session={session} depth={depth}
                attachOpen={attachOpenSet.has(session.id)} onToggleAttach={() => toggleAttach(session.id)} />
            ))}
          </div>
        )}
        {filter && !orphanRows.length && !sessions.some((s) => sessionTitle(s).toLowerCase().includes(filter)) && (
          <p className="px-2.5 py-3 text-xs text-muted-foreground">没有匹配“{query}”的会话。</p>
        )}
      </nav>
      <ManageProjectsDialog open={manageOpen} onOpenChange={setManageOpen} />
    </div>
  );
}
