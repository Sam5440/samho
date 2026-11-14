// 任务看板：当前工作区任务按状态分组（运行中 / 已完成 / 失败 / 已取消）；运行中任务可停止。

import { KanbanSquareIcon, SquareIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { EmptyHint } from '@/components/panels/shared';
import { stopTask, useTasks, type TaskSnapshot } from '@/components/panels/agents';
import { cn } from '@/lib/utils';

const COLUMNS = [
  { key: 'running', label: '运行中', variant: 'brand', dot: 'bg-brand' },
  { key: 'completed', label: '已完成', variant: 'success', dot: 'bg-success' },
  { key: 'failed', label: '失败', variant: 'destructive', dot: 'bg-destructive' },
  { key: 'cancelled', label: '已取消', variant: 'secondary', dot: 'bg-muted-foreground' },
] as const;

function TaskCard({ task, refresh }: { task: TaskSnapshot; refresh: () => void }) {
  const progress = task.progress || {};
  const title = task.result && typeof task.result === 'object' ? String(task.result.output || task.result.session_id || '').slice(0, 80) : '';
  return (
    <article data-status={task.status} className={cn('grid min-w-0 gap-1 rounded-md border bg-card p-2', task.status === 'failed' && 'border-destructive/40')}>
      <div className="flex items-center gap-1.5">
        <code className="font-mono text-[11px]">{String(task.task_id).slice(0, 8)}</code>
        {task.status === 'running' && <Spinner className="size-3 text-brand" />}
        {progress.model_id && <span className="truncate text-[11px] text-muted-foreground">{progress.model_id}</span>}
      </div>
      {title && <p className="line-clamp-2 text-xs text-muted-foreground">{title}</p>}
      {(progress.turn || progress.usage) && (
        <div className="flex gap-2 text-[11px] text-muted-foreground">
          {progress.turn ? <span>轮次 {progress.turn}</span> : null}
          {progress.usage ? <span>tokens {progress.usage.total_tokens ?? '–'}</span> : null}
        </div>
      )}
      {task.status === 'running' && (
        <Button variant="outline" size="xs" className="h-6 justify-self-start text-[11px]" onClick={() => stopTask(task.task_id, refresh)}>
          <SquareIcon className="fill-current" />停止任务
        </Button>
      )}
    </article>
  );
}

export function TasksPanel({ active }: { active: boolean }) {
  const { tasks, error, refresh } = useTasks(active);
  if (error) return <EmptyHint title="无法读取任务">{error}</EmptyHint>;
  if (!tasks) return <EmptyHint><Spinner /></EmptyHint>;
  if (!tasks.length) return <EmptyHint icon={KanbanSquareIcon} title="还没有任务记录">发送一条消息即会产生任务。</EmptyHint>;
  return (
    <div className="grid grid-cols-2 content-start gap-3 overflow-y-auto p-3">
      {COLUMNS.map((column) => {
        const list = tasks.filter((t) => t.status === column.key);
        return (
          <div key={column.key} data-status={column.key} className="grid min-w-0 content-start gap-1.5">
            <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
              <span className={cn('size-1.5 rounded-full', column.dot)} />{column.label}
              <Badge variant={column.variant} className="ml-auto px-1 py-0">{list.length}</Badge>
            </div>
            {list.length ? list.map((task) => <TaskCard key={task.task_id} task={task} refresh={refresh} />)
              : <p className="rounded-md border border-dashed py-3 text-center text-[11px] text-muted-foreground">—</p>}
          </div>
        );
      })}
    </div>
  );
}
