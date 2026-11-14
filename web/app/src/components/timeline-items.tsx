// 时间线条目：工具调用卡（注册表文案 + 四态）与思考块（运行计时 / 完成耗时）。

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  BrainIcon, CheckIcon, ChevronDownIcon, FilePenLineIcon, FileTextIcon, FolderTreeIcon, GlobeIcon,
  ListChecksIcon, PencilLineIcon, PlayIcon, SearchIcon, SparklesIcon, SquareTerminalIcon, WrenchIcon, XIcon, AsteriskIcon,
  type LucideIcon,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { reasoningHeading } from '@/lib/markdown';
import { Markdown, openWorkspaceFile } from '@/components/markdown';
import { previewWorkspaceFile } from '@/components/preview-dialog';
import type { ToolRow } from '@/lib/projection';
import { useTick } from '@/lib/hooks';
import { cn } from '@/lib/utils';

type Args = Record<string, unknown>;
type Pair = [string, unknown];
type Entry = { title: string; icon: LucideIcon; subtitle: (a: Args) => string; args: (a: Args) => (Pair | false)[] };

const defined = (pairs: (Pair | false)[]) => pairs.filter((p): p is Pair => Boolean(p) && (p as Pair)[1] !== undefined);

// 每个工具的展示文案（前端生成，metadata 只提供结构化事实）。
const registry: Record<string, Entry> = {
  read_file: {
    title: '读取文件', icon: FileTextIcon,
    subtitle: (a) => String(a.path || a.file_path || ''),
    args: (a) => [a.offset !== undefined && ['offset', a.offset], a.limit !== undefined && ['limit', a.limit]],
  },
  write_file: {
    title: '写入文件', icon: FilePenLineIcon,
    subtitle: (a) => String(a.path || a.file_path || ''),
    args: (a) => [a.content !== undefined && ['bytes', String(a.content).length], a.append !== undefined && ['append', a.append]],
  },
  edit_file: {
    title: '编辑文件', icon: PencilLineIcon,
    subtitle: (a) => String(a.path || ''),
    args: (a) => [
      a.old_string !== undefined && ['old', String(a.old_string).length],
      a.new_string !== undefined && ['new', String(a.new_string).length],
      a.replace_all !== undefined && ['all', a.replace_all],
    ],
  },
  load_skill: { title: '加载技能', icon: SparklesIcon, subtitle: (a) => String(a.name || ''), args: () => [] },
  list_files: {
    title: '浏览目录', icon: FolderTreeIcon,
    subtitle: (a) => String(a.path || '.'),
    args: (a) => [a.depth !== undefined && ['depth', a.depth], a.max_entries !== undefined && ['max_entries', a.max_entries]],
  },
  glob: {
    title: '匹配文件', icon: AsteriskIcon,
    subtitle: (a) => String(a.pattern || ''),
    args: (a) => [a.path !== undefined && ['path', a.path], a.max_entries !== undefined && ['max_entries', a.max_entries]],
  },
  grep: {
    title: '搜索内容', icon: SearchIcon,
    subtitle: (a) => String(a.pattern || ''),
    args: (a) => [a.path !== undefined && ['path', a.path], a.glob !== undefined && ['glob', a.glob], a.ignore_case !== undefined && ['ignore_case', a.ignore_case]],
  },
  webfetch: {
    title: '抓取网页', icon: GlobeIcon,
    subtitle: (a) => String(a.url || ''),
    args: (a) => [a.max_bytes !== undefined && ['max_bytes', a.max_bytes]],
  },
  todo_write: {
    title: '更新任务清单', icon: ListChecksIcon,
    subtitle: (a) => (Array.isArray(a.items) ? `${a.items.length} 项` : ''), args: () => [],
  },
  todo_read: { title: '读取任务清单', icon: ListChecksIcon, subtitle: () => '', args: () => [] },
  shell: { title: '执行命令', icon: SquareTerminalIcon, subtitle: (a) => String(a.command || ''), args: () => [] },
};

function describe(name: string, args: Args) {
  const entry = registry[name];
  if (entry) return { title: entry.title, icon: entry.icon, subtitle: entry.subtitle(args) || '', args: defined(entry.args(args)) };
  // 通用兜底（含 MCP 工具）：截取主要参数。
  const pairs = Object.entries(args || {}).slice(0, 3)
    .map(([key, value]): Pair => [key, typeof value === 'string' && value.length > 24 ? `${value.slice(0, 24)}…` : value]);
  return { title: name, icon: WrenchIcon, subtitle: '', args: pairs };
}

// 大输出钳制：max-h + 底部渐隐 + 展开/收起。
function ClampedPre({ text, tone, cursor }: { text: string; tone?: 'error'; cursor?: boolean }) {
  const ref = useRef<HTMLPreElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setOverflowing(el.scrollHeight > el.clientHeight + 2));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <div className="relative">
      <pre
        ref={ref}
        className={cn(
          'overflow-hidden rounded-md bg-muted/60 p-2.5 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap break-words',
          !expanded && 'max-h-64',
          overflowing && !expanded && '[mask-image:linear-gradient(to_bottom,black_calc(100%-3rem),transparent)]',
          tone === 'error' ? 'bg-destructive/8 text-destructive' : 'text-foreground/85',
        )}
      >
        {text}
        {cursor && <span aria-hidden="true" className="ml-0.5 inline-block h-[1em] w-[0.5em] translate-y-[0.15em] animate-pulse bg-current" />}
      </pre>
      {(overflowing || expanded) && (
        <div className={cn('flex justify-center', !expanded && 'absolute inset-x-0 bottom-1.5')}>
          <Button variant="secondary" size="xs" className="h-6 rounded-full border text-[11px] shadow-sm" onClick={() => setExpanded(!expanded)}>
            {expanded ? '收起' : '展开'}
          </Button>
        </div>
      )}
    </div>
  );
}

const MARKDOWN_FILE = /\.(md|markdown|mdx)$/i;
const HTML_FILE = /\.(html?|svg)$/i;
const FILE_TOOLS = new Set(['read_file', 'write_file', 'edit_file']);

// 文件类工具的富文本视图：Markdown 渲染、HTML 沙箱预览、在文件面板打开。
function fileView(row: ToolRow) {
  if (!FILE_TOOLS.has(row.name) || row.status !== 'completed') return null;
  const path = String(row.args.path || row.args.file_path || '');
  if (!path) return null;
  let content: string | null = null;
  if (row.name === 'write_file' && typeof row.args.content === 'string') content = row.args.content;
  if (row.name === 'read_file') {
    try { const parsed = JSON.parse(row.result); if (typeof parsed?.content === 'string') content = parsed.content; } catch { /* 非 JSON 输出 */ }
  }
  const dir = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
  return { path, dir, content, markdown: MARKDOWN_FILE.test(path), html: HTML_FILE.test(path) };
}

export function ToolCard({ row }: { row: ToolRow }) {
  const [open, setOpen] = useState(false);
  const [rendered, setRendered] = useState(true);
  const info = describe(row.name, row.args);
  const running = row.status === 'running';
  const failed = row.status === 'error';
  const Icon = info.icon;
  const hasInput = row.input && row.input !== '{}' && row.input !== 'undefined';
  const file = fileView(row);
  const showMarkdown = Boolean(file?.markdown && file.content !== null && rendered);

  return (
    <article data-status={failed ? 'error' : running ? 'running' : 'completed'}
      className={cn('overflow-hidden rounded-lg border bg-card', failed && 'border-destructive/40')}>
      <button
        type="button"
        aria-expanded={open}
        disabled={running}
        title={running ? '工具正在执行…' : info.subtitle || undefined}
        onClick={() => setOpen(!open)}
        className="flex min-h-9 w-full items-center gap-2 px-3 py-1.5 text-left text-[13px] outline-none hover:bg-accent/50 focus-visible:bg-accent/50 disabled:cursor-progress"
      >
        <Icon className={cn('size-4 shrink-0', failed ? 'text-destructive' : 'text-muted-foreground')} />
        <span className={cn('shrink-0 font-medium', running && 'shimmer')}>{info.title}</span>
        {info.subtitle && <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">{info.subtitle}</span>}
        <span className="hidden min-w-0 gap-1 overflow-hidden sm:flex">
          {info.args.map(([key, value]) => (
            <span key={key} className="shrink-0 rounded bg-muted px-1.5 font-mono text-[10.5px] text-muted-foreground">
              {key}=<b className="font-medium text-foreground/80">{String(value)}</b>
            </span>
          ))}
        </span>
        <span className="ml-auto flex shrink-0 items-center gap-2 text-[11px] text-muted-foreground">
          {row.latencyMs ? <span className="font-mono tabular-nums">{(row.latencyMs / 1000).toFixed(1)}s</span> : null}
          {running ? <Spinner className="size-3.5" />
            : failed ? <span className="flex items-center gap-1 text-destructive"><XIcon className="size-3.5" />失败</span>
              : <span className="flex items-center gap-1 text-success"><CheckIcon className="size-3.5" />完成</span>}
          {!running && <ChevronDownIcon className={cn('size-3.5 transition-transform', open && 'rotate-180')} />}
        </span>
      </button>
      {open && (
        <div className="grid gap-2.5 border-t px-3 py-2.5">
          {file && (
            <div className="flex flex-wrap items-center gap-1.5">
              {file.markdown && file.content !== null && (
                <Button variant="outline" size="xs" onClick={() => setRendered(!rendered)}>{rendered ? '查看源码' : '渲染 Markdown'}</Button>
              )}
              {file.html && (
                <Button variant="outline" size="xs" onClick={() => previewWorkspaceFile(file.path)}>
                  <PlayIcon />运行预览
                </Button>
              )}
              <Button variant="ghost" size="xs" onClick={() => openWorkspaceFile(file.path)}><FileTextIcon />在文件面板打开</Button>
            </div>
          )}
          {showMarkdown && file && (
            <div className="max-h-[32rem] overflow-auto rounded-md border bg-background px-3 py-2">
              <Markdown text={file.content || ''} baseDir={file.dir} />
            </div>
          )}
          {hasInput && !(showMarkdown && row.name === 'write_file') && (
            <div className="grid gap-1">
              <div className="text-[11px] font-medium text-muted-foreground">输入</div>
              <ClampedPre text={row.input} />
            </div>
          )}
          {row.result && !(showMarkdown && row.name === 'read_file') && (
            <div className="grid gap-1">
              <div className="text-[11px] font-medium text-muted-foreground">{failed ? '错误' : '输出'}</div>
              <ClampedPre text={row.result} tone={failed ? 'error' : undefined} />
            </div>
          )}
          {!hasInput && !row.result && <div className="text-xs text-muted-foreground">没有可展示的输入输出。</div>}
        </div>
      )}
    </article>
  );
}

export function ReasoningBlock({ text, running, startedAt, durationMs }: {
  text: string; running: boolean; startedAt?: number; durationMs?: number;
}) {
  const [open, setOpen] = useState(false);
  useTick(1000, running);
  const heading = useMemo(() => reasoningHeading(text), [text]);
  const duration = running
    ? (startedAt ? `${Math.max(0, Math.round((Date.now() - startedAt) / 1000))}s` : '')
    : (durationMs ? `${(Number(durationMs) / 1000).toFixed(1)}s` : '');

  return (
    <div className="rounded-lg border border-dashed bg-muted/30 text-sm">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left outline-none"
      >
        <BrainIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <span className={cn('shrink-0 text-xs font-medium', running ? 'shimmer' : 'text-muted-foreground')}>{running ? '思考中' : '已思考'}</span>
        {heading && <span className="min-w-0 truncate text-xs text-foreground/80">{heading}</span>}
        <span className="ml-auto flex shrink-0 items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
          {duration}
          <ChevronDownIcon className={cn('size-3.5 transition-transform', open && 'rotate-180')} />
        </span>
      </button>
      {open && <Markdown text={text} className="md-muted border-t border-dashed px-3 py-2 text-[13px]" />}
    </div>
  );
}
