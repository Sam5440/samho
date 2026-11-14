// Git 审查面板：变更清单（暂存/未暂存/未跟踪）+ 统一 diff + 提交 + 历史 + 分支切换。

import { useCallback, useEffect, useState } from 'react';
import {
  CheckIcon, ChevronDownIcon, GitBranchIcon, GitCommitHorizontalIcon, HistoryIcon, MinusIcon, MoreHorizontalIcon,
  PlusIcon, RefreshCwIcon, Undo2Icon, XIcon, FileDiffIcon,
} from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Textarea } from '@/components/ui/textarea';
import { Tip } from '@/components/ui/tooltip';
import { toast } from '@/components/ui/toaster';
import { confirmAction, promptText } from '@/components/dialogs';
import { EmptyHint, PanelToolbar, SectionLabel } from '@/components/panels/shared';
import { api, apiPost } from '@/lib/api';
import { relativeTime } from '@/lib/hooks';
import { cn } from '@/lib/utils';

const STATUS_LABEL: Record<string, string> = {
  M: '修改', A: '新增', D: '删除', R: '重命名', C: '复制', U: '冲突', '?': '未跟踪', '!': '丢失',
};
const STATUS_TONE: Record<string, string> = {
  M: 'text-warning', A: 'text-success', D: 'text-destructive', R: 'text-brand', '?': 'text-success', U: 'text-destructive',
};

type Change = { path: string; x: string; y: string; staged: boolean; untracked?: boolean };
type Commit = { short: string; subject: string; refs?: string; author: string; time: number };
type BranchInfo = { name: string; current?: boolean };

export function GitPanel({ active }: { active: boolean }) {
  const [branch, setBranch] = useState('');
  const [changes, setChanges] = useState<Change[]>([]);
  const [commits, setCommits] = useState<Commit[]>([]);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [selected, setSelected] = useState<{ path: string; staged: boolean } | null>(null);
  const [diff, setDiff] = useState<string | null>(null);
  const [showLog, setShowLog] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    try {
      const [status, log, branchData] = await Promise.all([
        api('/api/git/status'),
        api('/api/git/log?limit=20').catch(() => ({ commits: [] })),
        api('/api/git/branches').catch(() => ({ branches: [] })),
      ]);
      const list: Change[] = Array.isArray(status.changes) ? status.changes : [];
      setBranch(status.branch || '');
      setChanges(list);
      setCommits(log.commits || []);
      setBranches(branchData.branches || []);
      setError('');
      setSelected((sel) => (sel && list.some((c) => c.path === sel.path && c.staged === sel.staged) ? sel : null));
    } catch (err) {
      setError(`Git 不可用：${(err as Error).message}`);
    }
  }, []);

  useEffect(() => { if (active) refresh(); }, [active, refresh]);
  useEffect(() => {
    const onWs = () => { setSelected(null); setDiff(null); refresh(); };
    document.addEventListener('workspace-changed', onWs);
    return () => document.removeEventListener('workspace-changed', onWs);
  }, [refresh]);

  useEffect(() => {
    if (!selected) { setDiff(null); return; }
    let stale = false;
    setDiff(null);
    api(`/api/git/diff?path=${encodeURIComponent(selected.path)}&staged=${selected.staged}`)
      .then((data) => { if (!stale) setDiff(data.diff || ''); })
      .catch((err) => { if (!stale) setDiff(`差异读取失败：${err.message}`); });
    return () => { stale = true; };
  }, [selected]);

  const run = async (fn: () => Promise<unknown>, okText?: string, errPrefix = '操作失败') => {
    try { await fn(); await refresh(); if (okText) toast(okText); }
    catch (err) { toast(`${errPrefix}：${(err as Error).message}`, 'error'); }
  };

  const stageAll = () => {
    const paths = changes.filter((c) => !c.staged).map((c) => c.path);
    if (!paths.length) { toast('没有可暂存的变更'); return; }
    run(() => apiPost('/api/git/stage', { paths }), `已暂存 ${paths.length} 项`, '暂存失败');
  };

  const discard = async (change: Change) => {
    const ok = await confirmAction({
      title: '丢弃修改',
      description: `“${change.path}” 的未暂存修改将被永久丢弃。`,
      confirmText: '丢弃',
      destructive: true,
    });
    if (ok) run(() => apiPost('/api/git/discard', { paths: [change.path] }), '已丢弃修改', '丢弃失败');
  };

  const commit = async () => {
    const text = message.trim();
    if (!text) { toast('请填写提交说明', 'error'); return; }
    try {
      const result = await apiPost('/api/git/commit', { message: text, paths: [] });
      setMessage('');
      await refresh();
      toast(`已提交 ${result.hash || ''}`);
    } catch (err) { toast(`提交失败：${(err as Error).message}`, 'error'); }
  };

  const newBranch = async () => {
    const name = await promptText({ title: '从当前提交新建分支', label: '新分支名', confirmText: '创建并切换' });
    if (name) run(() => apiPost('/api/git/checkout', { branch: name, create: true }), `已创建并切换到 ${name}`, '创建失败');
  };

  const staged = changes.filter((c) => c.staged);
  const unstaged = changes.filter((c) => !c.staged);

  const changeRow = (change: Change) => {
    const code = change.untracked ? '?' : change.staged ? String(change.x) : String(change.y);
    const isActive = selected?.path === change.path && selected.staged === change.staged;
    return (
      <div
        key={`${change.staged ? 's' : 'w'}:${change.path}`}
        role="listitem"
        tabIndex={0}
        onClick={() => setSelected({ path: change.path, staged: change.staged })}
        onKeyDown={(e) => { if (e.key === 'Enter') setSelected({ path: change.path, staged: change.staged }); }}
        className={cn('group/git flex h-7 cursor-pointer items-center gap-2 rounded-md px-2 text-[13px] outline-none hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring/50', isActive && 'bg-sidebar-accent')}
      >
        <span className={cn('w-3 shrink-0 text-center font-mono text-[11px] font-semibold', STATUS_TONE[code] || 'text-muted-foreground')} title={STATUS_LABEL[code]}>
          {change.untracked ? 'U' : code}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-xs" title={change.path}>{change.path}</span>
        <span className="flex shrink-0 opacity-0 group-hover/git:opacity-100 group-focus-within/git:opacity-100">
          {change.staged ? (
            <Tip text="取消暂存"><Button variant="ghost" size="icon-xs" className="size-6" aria-label="取消暂存" onClick={(e) => { e.stopPropagation(); run(() => apiPost('/api/git/unstage', { paths: [change.path] }), undefined, '取消暂存失败'); }}><MinusIcon /></Button></Tip>
          ) : (
            <Tip text="暂存"><Button variant="ghost" size="icon-xs" className="size-6" aria-label="暂存" onClick={(e) => { e.stopPropagation(); run(() => apiPost('/api/git/stage', { paths: [change.path] }), undefined, '暂存失败'); }}><PlusIcon /></Button></Tip>
          )}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-xs" className="size-6" aria-label="变更操作" onClick={(e) => e.stopPropagation()}><MoreHorizontalIcon /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
              <DropdownMenuItem onSelect={() => setSelected({ path: change.path, staged: change.staged })}><FileDiffIcon />查看差异</DropdownMenuItem>
              {change.staged
                ? <DropdownMenuItem onSelect={() => run(() => apiPost('/api/git/unstage', { paths: [change.path] }), undefined, '取消暂存失败')}><MinusIcon />取消暂存</DropdownMenuItem>
                : <DropdownMenuItem onSelect={() => run(() => apiPost('/api/git/stage', { paths: [change.path] }), undefined, '暂存失败')}><PlusIcon />暂存</DropdownMenuItem>}
              {change.staged && <DropdownMenuItem onSelect={() => run(() => apiPost('/api/git/stage', { paths: [change.path] }), undefined, '暂存失败')}><PlusIcon />暂存（含工作区）</DropdownMenuItem>}
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={() => discard(change)}><Undo2Icon />丢弃修改…</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </div>
    );
  };

  return (
    <>
      <PanelToolbar>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="xs" aria-label="切换分支" className="min-w-0 font-mono">
              <GitBranchIcon /><span className="truncate">{branch || '（无分支）'}</span><ChevronDownIcon className="opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-80 w-56">
            <DropdownMenuLabel>分支</DropdownMenuLabel>
            {branches.map((b) => (
              <DropdownMenuItem key={b.name} onSelect={() => !b.current && run(() => apiPost('/api/git/checkout', { branch: b.name }), `已切换到 ${b.name}`, '切换失败')}>
                <span className="truncate font-mono text-xs">{b.name}</span>
                {b.current && <CheckIcon className="ml-auto text-brand" />}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={newBranch}><PlusIcon />新建分支…</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <div className="flex-1" />
        <Tip text="提交历史">
          <Button variant={showLog ? 'secondary' : 'ghost'} size="icon-xs" aria-label="历史" aria-expanded={showLog} onClick={() => setShowLog(!showLog)}><HistoryIcon /></Button>
        </Tip>
        <Button variant="outline" size="xs" onClick={stageAll}><PlusIcon />暂存全部</Button>
        <Tip text="刷新状态与历史"><Button variant="ghost" size="icon-xs" aria-label="刷新" onClick={refresh}><RefreshCwIcon /></Button></Tip>
      </PanelToolbar>

      <div role="list" aria-label="变更清单" className={cn('min-h-0 overflow-auto p-1.5', selected ? 'flex-[1_1_35%]' : 'flex-1')}>
        {error ? <EmptyHint icon={GitBranchIcon} title="Git 不可用">{error}</EmptyHint>
          : !changes.length ? <EmptyHint icon={CheckIcon} title="工作区干净">没有未提交变更。</EmptyHint>
            : (
              <>
                {staged.length > 0 && <SectionLabel className="px-2 pt-1 pb-0.5">已暂存 <Badge variant="secondary" className="px-1 py-0">{staged.length}</Badge></SectionLabel>}
                {staged.map(changeRow)}
                {unstaged.length > 0 && <SectionLabel className="px-2 pt-2 pb-0.5">工作区变更 <Badge variant="secondary" className="px-1 py-0">{unstaged.length}</Badge></SectionLabel>}
                {unstaged.map(changeRow)}
              </>
            )}
      </div>

      {selected && (
        <div aria-label="差异" className="flex min-h-0 flex-[1_1_65%] flex-col border-t">
          <div className="flex h-9 shrink-0 items-center gap-1.5 border-b px-2">
            <Badge variant={selected.staged ? 'success' : 'outline'}>{selected.staged ? '已暂存' : '工作区'}</Badge>
            <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{selected.path}</span>
            <Button variant="ghost" size="icon-xs" aria-label="关闭差异" onClick={() => setSelected(null)}><XIcon /></Button>
          </div>
          <div className="min-h-0 flex-1 overflow-auto py-1 font-mono text-[11.5px] leading-relaxed">
            {diff === null ? <p className="p-3 text-xs text-muted-foreground">正在读取差异…</p>
              : !diff.trim() ? <p className="p-3 text-xs text-muted-foreground">没有差异内容（未跟踪文件请先暂存）。</p>
                : diff.split('\n').map((line, i) => {
                  const meta = /^(diff |index |--- |\+\+\+ )/.test(line);
                  const added = !meta && line.startsWith('+');
                  const removed = !meta && line.startsWith('-');
                  const hunk = line.startsWith('@@');
                  return (
                    <div key={i} className={cn(
                      'px-3 whitespace-pre-wrap break-all',
                      added && 'bg-diff-add text-success',
                      removed && 'bg-diff-del text-destructive',
                      hunk && 'bg-brand/8 text-brand',
                      meta && 'text-muted-foreground',
                    )}>
                      {line || ' '}
                    </div>
                  );
                })}
          </div>
        </div>
      )}

      {showLog && (
        <div aria-label="提交历史" className="max-h-[40%] min-h-0 shrink-0 overflow-auto border-t p-1.5">
          <SectionLabel className="px-2 pb-1">最近提交</SectionLabel>
          {!commits.length && <p className="px-2 py-2 text-xs text-muted-foreground">还没有提交历史。</p>}
          {commits.map((c) => (
            <div key={c.short} className="flex items-center gap-2 rounded-md px-2 py-1 text-[13px] hover:bg-sidebar-accent">
              <GitCommitHorizontalIcon className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="shrink-0 font-mono text-[11px] text-brand">{c.short}</span>
              <span className="min-w-0 flex-1 truncate">{c.subject || '（无说明）'}</span>
              {c.refs && <Badge variant="outline" className="max-w-24 truncate">{c.refs}</Badge>}
              <span className="shrink-0 text-[11px] text-muted-foreground">{c.author} · {relativeTime(c.time * 1000)}</span>
            </div>
          ))}
        </div>
      )}

      <div className="flex shrink-0 items-end gap-2 border-t p-2">
        <Textarea
          rows={2}
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={(e) => { if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') commit(); }}
          placeholder="提交说明…（Ctrl+Enter 提交已暂存变更）"
          aria-label="提交说明"
          className="min-h-0 resize-none text-[13px]"
        />
        <Button size="sm" onClick={commit} disabled={!message.trim()}><CheckIcon />提交</Button>
      </div>
    </>
  );
}
