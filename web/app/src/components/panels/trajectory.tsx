// 轨迹面板：调用卡（sticky 摘要 + 输入 delta / 输出 section + 错误块）+ 按角色展开 + 搜索高亮 + 导出。
// 搜索高亮只用文本节点与 <mark> 元素，trace 正文（模型/工具/用户输入）不可信。

import { memo, useMemo, useReducer, useRef, useState } from 'react';
import { ActivityIcon, ChevronDownIcon, ChevronsDownUpIcon, ChevronsUpDownIcon, DownloadIcon, SearchIcon, SlidersHorizontalIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Switch } from '@/components/ui/switch';
import { Tip } from '@/components/ui/tooltip';
import { EmptyHint } from '@/components/panels/shared';
import { apiPath } from '@/lib/api';
import { createExpansion, roleLabel, TRAJECTORY_ROLES, type Expansion } from '@/lib/expansion';
import { conversationDelta, projectTrajectory, type Call } from '@/lib/projection';
import { useStore } from '@/lib/store';
import { cn } from '@/lib/utils';

const OMITTED = 'content omitted';
const ROLE_TEXT: Record<string, string> = { system: 'SYSTEM', user: 'USER', assistant: 'ASSISTANT', tool: 'TOOL', developer: 'SYSTEM' };
const ROLE_TONE: Record<string, string> = {
  user: 'text-brand', assistant: 'text-foreground', reasoning: 'text-muted-foreground',
  'tool-call': 'text-warning', 'tool-result': 'text-success', system: 'text-muted-foreground',
};

const normalize = (text: unknown) => String(text || '').replace(/\s+/g, ' ').toLowerCase();
const matches = (text: unknown, query: string) => Boolean(query) && normalize(text).includes(query);

// 归一化（空白折叠 + 小写）匹配映射回原文区间，命中区间包进 <mark>。
function Highlighted({ text, query }: { text: string; query: string }) {
  const raw = String(text ?? '');
  if (!query) return <>{raw}</>;
  const map: number[] = [];
  let norm = '';
  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i];
    if (/\s/.test(ch)) {
      if (norm.endsWith(' ')) continue;
      map.push(i); norm += ' ';
    } else { map.push(i); norm += ch.toLowerCase(); }
  }
  const parts: React.ReactNode[] = [];
  let cursor = 0;
  let at = norm.indexOf(query);
  while (at >= 0) {
    const start = map[at];
    const endIdx = at + query.length;
    const end = endIdx < map.length ? map[endIdx] : raw.length;
    if (start > cursor) parts.push(raw.slice(cursor, start));
    parts.push(<mark key={at}>{raw.slice(start, end)}</mark>);
    cursor = end;
    at = norm.indexOf(query, endIdx);
  }
  if (cursor < raw.length) parts.push(raw.slice(cursor));
  return <>{parts}</>;
}

function Pre({ text, query }: { text: string; query: string }) {
  return (
    <pre className="max-h-64 overflow-auto rounded-md bg-muted/50 p-2 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-words text-foreground/85">
      <Highlighted text={text} query={query} />
    </pre>
  );
}

function MessageRow({ role, visualRole, text, k, expansion, query, pills = [], label, onToggle }: {
  role: string; visualRole: string; text: string; k: string; expansion: Expansion; query: string;
  pills?: string[]; label?: string; onToggle: () => void;
}) {
  const open = expansion.isOpen(k, visualRole);
  const preview = String(text || '').replace(/\s+/g, ' ').trim().slice(0, 160);
  return (
    <div className="grid grid-cols-[4.75rem_minmax(0,1fr)] items-start gap-1.5" data-role={visualRole}>
      <div className={cn('pt-1 font-mono text-[9.5px] font-semibold tracking-wider', ROLE_TONE[visualRole])}>
        {ROLE_TEXT[role] || visualRole.toUpperCase()}
      </div>
      <div className="min-w-0">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => { expansion.toggle(k, visualRole); onToggle(); }}
          className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-[11.5px] hover:bg-accent/60"
        >
          {label && <span className="shrink-0 rounded-sm border px-1 text-[10px] text-muted-foreground">{label}</span>}
          {pills.map((p) => <span key={p} className="max-w-[40%] shrink-0 truncate rounded-sm bg-muted px-1 font-mono text-[10px]">{p}</span>)}
          <span className="min-w-0 flex-1 truncate text-muted-foreground">{preview || '—'}</span>
          <ChevronDownIcon className={cn('size-3 shrink-0 text-muted-foreground transition-transform', open && 'rotate-180')} />
        </button>
        {open && (
          <div className="grid gap-1 px-1.5 pb-1.5">
            {typeof text === 'string' && text.includes(OMITTED) ? (
              <div className="flex items-center gap-2 py-1">
                <Badge variant="warning">内容开关关闭</Badge>
                <span className="text-xs text-muted-foreground">运行时未记录消息内容（log_content）。</span>
              </div>
            ) : <Pre text={text} query={query} />}
          </div>
        )}
      </div>
    </div>
  );
}

function OmittedNote() {
  return (
    <div className="flex flex-wrap items-center gap-1.5 py-0.5">
      <Badge variant="warning">内容开关关闭</Badge>
      <span className="text-[11px] text-muted-foreground">运行时未记录消息正文（log_content = false）。</span>
    </div>
  );
}

const CallCard = memo(function CallCard({ call, index, previous, expansion, query, version, onToggle }: {
  call: Call; index: number; previous: Call | null; expansion: Expansion; query: string; version: number; onToggle: () => void;
}) {
  void version;
  const usage = call.usage || {};
  const delta = conversationDelta(previous ? previous.request : null, call.request);
  const output = call.output || {};
  const reasoning = typeof output.reasoning_content === 'string' ? output.reasoning_content : '';
  const text = typeof output.content === 'string' ? output.content : '';
  const toolCalls = Array.isArray(output.tool_calls) ? output.tool_calls : [];
  const hit = query && (
    matches(call.modelId, query)
    || call.request.some((m) => matches(typeof m.content === 'string' ? m.content : JSON.stringify(m.content), query))
    || matches(output.content, query)
    || toolCalls.some((t: any) => matches(t?.function?.name, query) || matches(t?.function?.arguments, query)) // eslint-disable-line @typescript-eslint/no-explicit-any
    || matches(call.error, query)
  );
  const finishVariant = call.error ? 'destructive' : call.finishReason === 'tool_calls' ? 'brand' : 'success';
  const hasOutput = reasoning.trim() || text.trim() || toolCalls.length || call.error;

  return (
    <article data-trajectory-call="" className={cn('rounded-lg border bg-card', hit && 'border-warning/60 ring-1 ring-warning/30')}>
      <div className="sticky top-0 z-[1] flex items-center gap-2 rounded-t-lg border-b bg-card/95 px-2.5 py-1.5 text-[11px] backdrop-blur">
        <span className="font-mono font-semibold text-muted-foreground tabular-nums">{String(index + 1).padStart(2, '0')}</span>
        <span className="min-w-0 truncate font-mono font-medium">{call.modelId || '—'}</span>
        {call.finishReason && <Badge variant={finishVariant} className="font-mono">{call.finishReason}</Badge>}
        {call.error && <Badge variant="destructive">错误</Badge>}
        <span className="ml-auto shrink-0 font-mono text-muted-foreground tabular-nums">
          {usage.input_tokens !== undefined && `IN ${usage.input_tokens} · `}
          {usage.output_tokens !== undefined && `OUT ${usage.output_tokens} · `}
          {call.latencyMs ? `${(call.latencyMs / 1000).toFixed(1)}s · ` : ''}
          {call.startedAt ? new Date(call.startedAt).toLocaleTimeString('zh-CN', { hour12: false }) : ''}
        </span>
      </div>
      <div className="grid gap-1 px-2.5 py-2">
        <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
          输入 {!call.omitted && <Badge variant="outline" className="px-1 py-0 text-[10px]">{delta.length} 条新增</Badge>}
        </div>
        {call.omitted && <OmittedNote />}
        {!call.omitted && !delta.length && <div className="text-[11px] text-muted-foreground">与上一次调用相比没有新增消息。</div>}
        {delta.map((message, i) => {
          const role = String(message.role || 'unknown').toLowerCase();
          const body = typeof message.content === 'string' ? message.content : JSON.stringify(message.content, null, 2);
          const visualRole = role === 'user' ? 'user' : role === 'assistant' ? 'assistant' : role === 'tool' ? 'tool-result' : 'system';
          return (
            <MessageRow key={i} role={role} visualRole={visualRole} text={body} k={`${call.id}:input:${i}`}
              expansion={expansion} query={query} onToggle={onToggle}
              pills={role === 'tool' && message.tool_call_id ? [String(message.tool_call_id).replace(/^call_/, '')] : []} />
          );
        })}
      </div>
      <div className="grid gap-1 border-t px-2.5 py-2">
        <div className="text-[11px] font-medium text-muted-foreground">输出</div>
        {reasoning.trim() && (
          <MessageRow role="assistant" visualRole="reasoning" text={reasoning} k={`${call.id}:output:reasoning`} label="思考"
            expansion={expansion} query={query} onToggle={onToggle} />
        )}
        {(text.trim() || (!call.error && !reasoning.trim() && !toolCalls.length && call.output)) && (
          <MessageRow role="assistant" visualRole="assistant" text={text} k={`${call.id}:output:text`}
            expansion={expansion} query={query} onToggle={onToggle} />
        )}
        {toolCalls.map((toolCall: any, i: number) => { // eslint-disable-line @typescript-eslint/no-explicit-any
          const input = typeof toolCall?.function?.arguments === 'string'
            ? toolCall.function.arguments : JSON.stringify(toolCall?.function?.arguments ?? {}, null, 2);
          return (
            <MessageRow key={i} role="tool" visualRole="tool-call" text={input} k={`${call.id}:output:tool-call:${i}`} label="调用"
              pills={[toolCall?.function?.name || 'tool', ...(toolCall?.id ? [String(toolCall.id).replace(/^call_/, '')] : [])]}
              expansion={expansion} query={query} onToggle={onToggle} />
          );
        })}
        {call.error && (
          <div className="rounded-md border border-destructive/40 bg-destructive/5 p-2">
            <div className="mb-1 text-[11px] font-semibold text-destructive">调用失败</div>
            <pre className="font-mono text-[11px] whitespace-pre-wrap break-words text-destructive"><Highlighted text={call.error} query={query} /></pre>
          </div>
        )}
        {!hasOutput && (
          !call.responded ? <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground"><span className="shimmer">等待模型响应…</span></div>
            : call.omitted ? <OmittedNote />
              : <div className="text-[11px] text-muted-foreground">该调用没有可展示的输出记录。</div>
        )}
      </div>
    </article>
  );
});

export function TrajectoryPanel() {
  const events = useStore((s) => s.trace);
  const traceId = useStore((s) => s.traceId);
  const expansion = useRef(createExpansion()).current;
  const [version, bump] = useReducer((v: number) => v + 1, 0);
  const [rawQuery, setRawQuery] = useState('');
  const query = rawQuery.trim().toLowerCase();
  const calls = useMemo(() => projectTrajectory(events), [events]);

  const totalIn = calls.reduce((sum, c) => sum + Number(c.usage?.input_tokens || 0), 0);
  const totalOut = calls.reduce((sum, c) => sum + Number(c.usage?.output_tokens || 0), 0);
  const models = [...new Set(calls.map((c) => c.modelId).filter(Boolean))];
  const kinds = useMemo(() => {
    const map = new Map<string, number>();
    for (const event of events) map.set(event.kind, (map.get(event.kind) || 0) + 1);
    return [...map];
  }, [events]);

  const setAll = (expanded: boolean) => { TRAJECTORY_ROLES.forEach((r) => expansion.setRole(r, expanded)); bump(); };

  return (
    <>
      <div className="grid shrink-0 gap-2 border-b p-2">
        <div className="flex items-center gap-1">
          <span className="pl-1 text-sm font-semibold">模型轨迹</span>
          <div className="flex-1" />
          <Tip text="全部展开"><Button variant="ghost" size="icon-xs" aria-label="全部展开" onClick={() => setAll(true)}><ChevronsUpDownIcon /></Button></Tip>
          <Tip text="全部收起"><Button variant="ghost" size="icon-xs" aria-label="全部收起" onClick={() => setAll(false)}><ChevronsDownUpIcon /></Button></Tip>
          <Popover>
            <Tip text="按角色设置展开">
              <PopoverTrigger asChild>
                <Button variant="ghost" size="icon-xs" aria-label="按角色设置展开"><SlidersHorizontalIcon /></Button>
              </PopoverTrigger>
            </Tip>
            <PopoverContent align="end" className="w-52 p-1.5">
              <div className="px-2 pt-1 pb-1.5 text-xs font-medium text-muted-foreground">默认展开</div>
              {TRAJECTORY_ROLES.map((role) => (
                <label key={role} className="flex cursor-pointer items-center justify-between rounded-md px-2 py-1.5 text-sm hover:bg-accent">
                  {roleLabel(role)}
                  <Switch checked={expansion.getRole(role)} onCheckedChange={(v) => { expansion.setRole(role, v); bump(); }} />
                </label>
              ))}
            </PopoverContent>
          </Popover>
          <Button variant="outline" size="xs" disabled={!traceId}
            onClick={() => traceId && window.open(apiPath(`/api/traces/${encodeURIComponent(traceId)}/export`), '_blank', 'noopener')}>
            <DownloadIcon />导出
          </Button>
        </div>
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input value={rawQuery} onChange={(e) => setRawQuery(e.target.value)} placeholder="搜索轨迹内容…" spellCheck={false} aria-label="搜索轨迹内容" className="h-8 pl-8 text-xs" />
        </div>
        {calls.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
            <span className="tabular-nums">{calls.length} 次调用</span>
            <span className="font-mono tabular-nums">IN {totalIn} · OUT {totalOut}</span>
            {models.map((m) => <Badge key={m} variant="outline" className="font-mono font-normal">{m}</Badge>)}
          </div>
        )}
      </div>
      <div className="grid min-h-0 flex-1 content-start gap-2 overflow-y-auto p-2">
        {!calls.length && !events.length && (
          <EmptyHint icon={ActivityIcon} title="等待执行信号">从任务到结果，完整记录每一次模型调用。</EmptyHint>
        )}
        {!calls.length && events.length > 0 && (
          <EmptyHint icon={ActivityIcon} title="本次执行没有产生模型调用">
            任务在模型调用前结束，仅有执行事件：
            <div className="mt-2 flex flex-wrap justify-center gap-1">
              {kinds.map(([kind, count]) => <Badge key={kind} variant="outline" className="font-mono">{kind} ×{count}</Badge>)}
            </div>
          </EmptyHint>
        )}
        {calls.map((call, index) => (
          <CallCard key={call.id} call={call} index={index} previous={calls[index - 1] || null}
            expansion={expansion} query={query} version={version} onToggle={bump} />
        ))}
      </div>
    </>
  );
}
