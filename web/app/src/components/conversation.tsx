// 对话区：会话头 + 时间线（消息/思考/工具卡 + live turn）+ 输入区。

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ArrowDownIcon, GitForkIcon, SquareTerminalIcon, FileTextIcon, SearchCodeIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Spinner } from '@/components/ui/spinner';
import { Composer } from '@/components/composer';
import { ReasoningBlock, ToolCard } from '@/components/timeline-items';
import { forkSession, taskPhase } from '@/lib/controller';
import { Markdown } from '@/components/markdown';
import { projectTimeline, type LiveSnapshot, type LiveTool, type Row, type ToolRow } from '@/lib/projection';
import { currentViewKey, sessionTitle, useStore } from '@/lib/store';
import { cn } from '@/lib/utils';

const NEAR_BOTTOM = 80;

function TurnDivider({ label }: { label: string }) {
  return (
    <div role="separator" className="flex items-center gap-3 pt-2 text-[11px] text-muted-foreground">
      <span className="h-px flex-1 bg-border" />
      <span>{label}</span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

function RowView({ row }: { row: Row }) {
  switch (row.type) {
    case 'turn':
      return <TurnDivider label={`第 ${row.turn} 轮`} />;
    case 'user':
      return (
        <div className="flex justify-end">
          <div className="max-w-[85%] rounded-2xl rounded-br-md bg-secondary px-4 py-2.5 text-sm leading-relaxed whitespace-pre-wrap break-words text-secondary-foreground">
            {row.text}
          </div>
        </div>
      );
    case 'text':
      return (
        <article className="space-y-2">
          <Markdown text={row.text} />
          {row.model && <Badge variant="outline" className="font-mono font-normal">{row.model}</Badge>}
        </article>
      );
    case 'reasoning':
      return <ReasoningBlock text={row.text} running={false} durationMs={row.durationMs} />;
    case 'tool':
      return <ToolCard row={row} />;
    case 'note':
      return (
        <div className={cn(
          'rounded-md border-l-2 px-3 py-1.5 text-sm',
          row.tone === 'error' ? 'border-destructive bg-destructive/5 text-destructive' : 'border-border text-muted-foreground',
        )}>
          {row.text}
        </div>
      );
  }
}

function liveToolRows(live: LiveSnapshot): ToolRow[] {
  const rows: ToolRow[] = [];
  const shown = new Set<string>();
  const statusOf = (snapshot?: LiveTool) =>
    snapshot ? (snapshot.status === 'done' ? 'completed' : snapshot.status === 'error' ? 'error' : 'running') : 'running';
  for (const call of live.toolCalls || []) {
    const id = String(call?.id || '');
    if (!id || shown.has(id)) continue;
    shown.add(id);
    const snapshot = live.tools.get(id);
    let args: Record<string, unknown> = {};
    let input = '';
    const raw = call?.function?.arguments;
    if (typeof raw === 'string') { input = raw; try { args = JSON.parse(raw); } catch { args = { _raw: raw }; } }
    else if (raw) { args = raw; input = JSON.stringify(raw, null, 2); }
    rows.push({
      type: 'tool', key: `t:${id}`, callId: id, name: call?.function?.name || snapshot?.name || 'tool',
      args, input, status: statusOf(snapshot), result: snapshot?.result || '', latencyMs: snapshot?.latencyMs,
    });
  }
  for (const [id, snapshot] of live.tools) {
    if (shown.has(id)) continue;
    rows.push({
      type: 'tool', key: `t:${id}`, callId: id, name: snapshot.name, args: snapshot.args, input: snapshot.input,
      status: statusOf(snapshot), result: snapshot.result, latencyMs: snapshot.latencyMs,
    });
  }
  return rows;
}

// live turn：流式思考 + 流式正文 + 运行中的工具调用。正文不进 aria-live（阶段摘要由 sr-only status 播报）。
function LiveTurn({ live }: { live: LiveSnapshot }) {
  const tools = liveToolRows(live);
  const empty = !live.reasoning.trim() && !live.text.trim() && !tools.length;
  return (
    <section className="space-y-4">
      {live.turn ? <TurnDivider label={`第 ${live.turn} 轮${(live.attempt || 1) > 1 ? ` · 重试 ${live.attempt}` : ''}`} /> : null}
      {live.reasoning.trim() && <ReasoningBlock text={live.reasoning} running startedAt={live.startedAt} />}
      {live.text.trim() && (
        <article className="space-y-2">
          <Markdown text={live.text} />
          <div className="flex items-center gap-1.5">
            {live.modelId && <Badge variant="outline" className="font-mono font-normal">{live.modelId}</Badge>}
            <Badge variant="brand"><Spinner className="size-3" />正在生成</Badge>
          </div>
        </article>
      )}
      {tools.map((row) => <ToolCard key={row.key} row={row} />)}
      {empty && (
        <div className="flex items-center gap-2 text-sm">
          <Spinner className="text-brand" />
          <span className="shimmer">智能体正在执行任务…</span>
        </div>
      )}
    </section>
  );
}

function EmptyState() {
  const simulation = useStore((s) => s.simulation);
  const suggestions = [
    { icon: FileTextIcon, text: '/demo read README.md', hint: '读取文件（模拟模式下的真实工具调用）' },
    { icon: SearchCodeIcon, text: '总结这个仓库的架构与模块划分', hint: '探索并概括代码库' },
    { icon: SquareTerminalIcon, text: '调试：找出最近一次测试失败的原因', hint: '推理层级任务' },
  ];
  return (
    <div className="mx-auto flex h-full max-w-2xl flex-col items-center justify-center px-6 pb-24 text-center">
      <h2 className="text-xl font-semibold tracking-tight">从一个任务开始</h2>
      <p className="mt-1.5 text-sm text-muted-foreground">
        描述你要完成的事，智能体会规划、调用工具并记录完整轨迹。
        {simulation && <><br />当前为模拟模式，输出会标注 <code className="font-mono text-xs">[DEMO simulation]</code>。</>}
      </p>
      <div className="mt-6 grid w-full gap-2 sm:grid-cols-3">
        {suggestions.map(({ icon: Icon, text, hint }) => (
          <button
            key={text}
            type="button"
            onClick={() => document.dispatchEvent(new CustomEvent('composer-fill', { detail: text }))}
            className="group rounded-xl border bg-card p-3 text-left transition-colors hover:border-ring/50 hover:bg-accent/50"
          >
            <Icon className="mb-2 size-4 text-muted-foreground group-hover:text-brand" />
            <div className="line-clamp-2 text-[13px] font-medium">{text}</div>
            <div className="mt-1 line-clamp-1 text-[11px] text-muted-foreground">{hint}</div>
          </button>
        ))}
      </div>
    </div>
  );
}

function ConversationHeader() {
  const sessionId = useStore((s) => s.currentSessionId);
  const session = useStore((s) => s.sessions.find((x) => x?.id === s.currentSessionId));
  const task = useStore((s) => s.task);
  const maxTurns = useStore((s) => Number(s.limits?.max_turns) || 0);
  const projects = useStore((s) => s.projects);
  const project = session?.project_id ? projects.find((p) => p.id === session.project_id) : null;
  const turn = Number(task?.progress?.turn || 0);

  return (
    <div className="flex h-12 shrink-0 items-center gap-3 border-b px-4">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          {project && (
            <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
              <span className="size-2 rounded-full" style={{ background: project.color || 'var(--muted-foreground)' }} />
              {project.name}
              <span className="text-border">/</span>
            </span>
          )}
          <h1 id="conversation-title" className="truncate text-sm font-semibold">
            {sessionId ? sessionTitle(session || { id: sessionId }) : '新任务'}
          </h1>
          {session?.archived && <Badge variant="outline">已归档</Badge>}
        </div>
        <div id="conversation-sub" className="truncate text-[11px] text-muted-foreground">
          {sessionId
            ? `会话 ${sessionId.slice(0, 8)} · ${session?.archived ? '发送消息将自动取消归档' : '继续对话将追加到本会话'}`
            : '草稿 · 发送首条消息后落库'}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {task ? (
          <div id="task-state" className="flex items-center gap-2 text-xs text-brand">
            <Spinner className="size-3.5" />
            <span>{taskPhase(task)}{turn ? ` · 第 ${turn} 轮` : ''}</span>
            <div
              id="task-progress"
              role="progressbar"
              aria-label="任务轮次进度"
              aria-valuemin={0}
              aria-valuemax={maxTurns || undefined}
              aria-valuenow={maxTurns ? turn : undefined}
              className="relative hidden h-1 w-20 overflow-hidden rounded-full bg-muted sm:block"
            >
              {maxTurns
                ? <div className="h-full rounded-full bg-brand transition-[width] duration-500" style={{ width: `${Math.min(100, (turn / maxTurns) * 100)}%` }} />
                : <div className="absolute inset-y-0 w-1/3 animate-pulse rounded-full bg-brand" />}
            </div>
          </div>
        ) : (
          <span id="task-state" className="text-xs text-muted-foreground">就绪</span>
        )}
        <Button
          id="fork-session"
          variant="outline"
          size="xs"
          disabled={!sessionId || Boolean(task)}
          onClick={() => sessionId && forkSession(sessionId)}
        >
          <GitForkIcon />分支
        </Button>
      </div>
    </div>
  );
}

export function Conversation() {
  const viewKey = useStore((s) => currentViewKey(s));
  const messages = useStore((s) => s.transcripts[viewKey]);
  const task = useStore((s) => s.task);
  const liveToolsRecord = useStore((s) => s.liveTools);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [showJump, setShowJump] = useState(false);

  // 只有任务属于当前视图时才渲染 live turn（任务视图 key 或其落库会话）。
  const taskInView = task && (task.viewKey === viewKey || (task.sessionId && `session:${task.sessionId}` === viewKey));
  const live: LiveSnapshot | null = useMemo(() => {
    if (!taskInView || !task) return null;
    const progress = task.progress || {};
    return {
      running: true,
      turn: progress.turn,
      attempt: progress.attempt,
      modelId: progress.model_id,
      reasoning: progress.reasoning || '',
      text: progress.text || '',
      tools: new Map(Object.entries(liveToolsRecord)),
      toolCalls: Array.isArray(progress.tool_calls) ? progress.tool_calls : [],
      startedAt: Number(progress.started_at) || task.startedAt,
    };
  }, [taskInView, task, liveToolsRecord]);

  const rows = useMemo(() => projectTimeline(messages || [], live), [messages, live]);
  const empty = rows.length === 0 && !live;

  // 切换视图时重新钉底。
  useEffect(() => { pinned.current = true; }, [viewKey]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  });

  useEffect(() => {
    // 选中文本时不自动跟随。
    const onSelection = () => {
      const selection = document.getSelection();
      if (selection && !selection.isCollapsed && scrollRef.current?.contains(selection.anchorNode)) pinned.current = false;
    };
    document.addEventListener('selectionchange', onSelection);
    return () => document.removeEventListener('selectionchange', onSelection);
  }, []);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinned.current = el.scrollTop + el.clientHeight >= el.scrollHeight - NEAR_BOTTOM;
    setShowJump(!pinned.current);
  };

  return (
    <>
      <ConversationHeader />
      <div id="timeline" ref={scrollRef} onScroll={onScroll} aria-label="任务对话" className="relative min-h-0 flex-1 overflow-y-auto">
        {empty ? <EmptyState /> : (
          <div className="mx-auto flex max-w-3xl flex-col gap-4 px-6 pt-6 pb-10">
            {rows.map((row) => <RowView key={row.key} row={row} />)}
            {live && <LiveTurn live={live} />}
          </div>
        )}
        {showJump && (
          <Button
            variant="secondary"
            size="xs"
            className="sticky bottom-4 left-1/2 -translate-x-1/2 rounded-full border shadow-md"
            onClick={() => {
              pinned.current = true;
              setShowJump(false);
              const el = scrollRef.current;
              if (el) el.scrollTop = el.scrollHeight;
            }}
          >
            <ArrowDownIcon />回到底部
          </Button>
        )}
      </div>
      <Composer />
    </>
  );
}

