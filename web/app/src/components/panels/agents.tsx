// 子代理面板：运行中的智能体执行（每任务一个 agent loop）。来源 GET /api/tasks，2.5s 轮询。

import { useState } from 'react';
import { BotIcon, SquareArrowOutUpRightIcon, SquareIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { toast } from '@/components/ui/toaster';
import { EmptyHint, SectionLabel } from '@/components/panels/shared';
import { api } from '@/lib/api';
import { cancelTaskById, loadSession } from '@/lib/controller';
import { usePolling } from '@/lib/hooks';
import { cn } from '@/lib/utils';

/* eslint-disable @typescript-eslint/no-explicit-any */
export type TaskSnapshot = { task_id: string; status: string; result?: any; error?: string; progress?: any };

const STATUS_LABELS: Record<string, string> = { running: '执行中', completed: '已完成', done: '已完成', failed: '失败', cancelled: '已取消' };

export function useTasks(active: boolean) {
  const [tasks, setTasks] = useState<TaskSnapshot[] | null>(null);
  const [error, setError] = useState('');
  const refresh = async () => {
    try {
      const body = await api('/api/tasks');
      setTasks(Array.isArray(body?.tasks) ? body.tasks : []);
      setError('');
    } catch (err) { setError(`加载失败：${(err as Error).message}`); }
  };
  usePolling(refresh, 2500, active);
  return { tasks, error, refresh };
}

export async function stopTask(taskId: string, refresh: () => void) {
  try {
    await cancelTaskById(taskId);
    toast('已请求停止');
    refresh();
  } catch (error) { toast(`停止失败：${(error as Error).message}`, 'error'); }
}

function AgentCard({ task, refresh }: { task: TaskSnapshot; refresh: () => void }) {
  const progress = task.progress || {};
  const running = task.status === 'running';
  const modelId = progress.model_id || task.result?.model_id;
  return (
    <article data-status={task.status} className={cn('grid gap-1.5 rounded-lg border bg-card p-2.5', running && 'border-brand/40', task.status === 'failed' && 'border-destructive/40')}>
      <div className="flex items-center justify-between gap-2">
        <span className="flex min-w-0 items-center gap-1.5 text-[13px] font-medium">
          {running ? <Spinner className="size-3.5 text-brand" /> : <BotIcon className="size-3.5 text-muted-foreground" />}
          {modelId ? <code className="truncate font-mono text-xs">{modelId}</code> : '代理'}
        </span>
        <Badge variant={running ? 'brand' : task.status === 'failed' ? 'destructive' : task.status === 'completed' ? 'success' : 'secondary'}>
          {STATUS_LABELS[task.status] || task.status}
        </Badge>
      </div>
      <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
        <span>{running ? `轮次 ${progress.turn ?? '–'} · 尝试 ${progress.attempt ?? 1}` : `轮次 ${task.result?.turns ?? '–'} · $${Number(task.result?.cost_usd ?? 0).toFixed(4)}`}</span>
        {progress.trace_id && <code className="font-mono">{String(progress.trace_id).slice(0, 8)}</code>}
      </div>
      {running && String(progress.text || '').trim() && (
        <p className="line-clamp-2 text-xs text-muted-foreground">{String(progress.text).slice(-160)}</p>
      )}
      {task.status === 'failed' && task.error && <p className="line-clamp-3 text-xs text-destructive">{String(task.error).slice(0, 200)}</p>}
      {running && (
        <div className="flex gap-1">
          {progress.session_id && (
            <Button variant="ghost" size="xs" onClick={() => loadSession(String(progress.session_id))}><SquareArrowOutUpRightIcon />打开会话</Button>
          )}
          <Button variant="ghost" size="xs" className="text-destructive hover:text-destructive" onClick={() => stopTask(task.task_id, refresh)}>
            <SquareIcon className="fill-current" />停止
          </Button>
        </div>
      )}
    </article>
  );
}

export function AgentsPanel({ active }: { active: boolean }) {
  const { tasks, error, refresh } = useTasks(active);
  if (error) return <EmptyHint title="无法读取任务">{error}</EmptyHint>;
  if (!tasks) return <EmptyHint><Spinner /></EmptyHint>;
  const running = tasks.filter((t) => t.status === 'running');
  const finished = tasks.filter((t) => t.status !== 'running').slice(0, 12);
  const section = (label: string, list: TaskSnapshot[], empty: string) => (
    <div className="grid gap-1.5">
      <SectionLabel>{label}<Badge variant="secondary" className="px-1 py-0">{list.length}</Badge></SectionLabel>
      {list.length ? list.map((task) => <AgentCard key={task.task_id} task={task} refresh={refresh} />)
        : <p className="text-xs text-muted-foreground">{empty}</p>}
    </div>
  );
  return (
    <div className="grid content-start gap-4 overflow-y-auto p-3">
      {section('运行中', running, '没有正在执行的子代理。')}
      {section('最近完成', finished, '暂无历史执行。')}
    </div>
  );
}
