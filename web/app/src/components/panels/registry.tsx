// 注册表面板：对象 / MCP 服务 / 技能。可折叠卡片，详情为脱敏后的 JSON / 指令摘录。

import { useState, type ReactNode } from 'react';
import { BlocksIcon, ChevronRightIcon, PlugIcon, PuzzleIcon, RefreshCwIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Tip } from '@/components/ui/tooltip';
import { EmptyHint, PanelToolbar } from '@/components/panels/shared';
import { refreshSkills } from '@/lib/controller';
import { useStore } from '@/lib/store';
import { cn } from '@/lib/utils';

function RegistryCard({ kind, kindVariant = 'secondary', name, description, meta, detail }: {
  kind: string; kindVariant?: 'secondary' | 'success' | 'destructive' | 'warning' | 'brand';
  name: string; description?: string; meta?: string; detail?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="rounded-lg border bg-card">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="flex w-full items-center gap-2 px-2.5 py-2 text-left">
        <ChevronRightIcon className={cn('size-3.5 shrink-0 text-muted-foreground transition-transform', open && 'rotate-90')} />
        <Badge variant={kindVariant} className="shrink-0">{kind}</Badge>
        <span className="min-w-0 flex-1 truncate font-mono text-xs">{name || '未命名'}</span>
      </button>
      {(description || meta) && (
        <div className="grid gap-0.5 px-2.5 pb-2 pl-8">
          {description && <p className={cn('text-xs text-muted-foreground', !open && 'line-clamp-2')}>{description}</p>}
          {meta && <p className="truncate font-mono text-[11px] text-muted-foreground/80">{meta}</p>}
        </div>
      )}
      {open && detail && (
        <pre className="mx-2.5 mb-2.5 max-h-72 overflow-auto rounded-md bg-muted/50 p-2 font-mono text-[11px] leading-relaxed whitespace-pre-wrap break-words">{detail}</pre>
      )}
    </div>
  );
}

function FilteredList({ count, label, filter, setFilter, children, actions }: {
  count: number; label: string; filter: string; setFilter: (v: string) => void; children: ReactNode; actions?: ReactNode;
}) {
  return (
    <>
      <PanelToolbar>
        <span className="pl-1 text-xs text-muted-foreground">{label} · <span className="tabular-nums">{count}</span></span>
        <div className="flex-1" />
        {actions}
        <Input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="筛选…" aria-label={`筛选${label}`} className="h-7 w-32 text-xs" />
      </PanelToolbar>
      <div className="grid min-h-0 flex-1 content-start gap-1.5 overflow-y-auto p-2">{children}</div>
    </>
  );
}

const includes = (q: string, ...parts: unknown[]) => !q || parts.some((p) => String(p || '').toLowerCase().includes(q));

export function ObjectsPanel() {
  const objects = useStore((s) => s.objects);
  const [filter, setFilter] = useState('');
  const q = filter.trim().toLowerCase();
  const list = objects.filter((o) => includes(q, o.id, o.name, o.kind, o.description));
  return (
    <FilteredList count={objects.length} label="对象注册表" filter={filter} setFilter={setFilter}>
      {!objects.length && <EmptyHint icon={BlocksIcon} title="对象注册表为空" />}
      {list.map((object) => (
        <RegistryCard
          key={String(object.id || object.name)}
          kind={String(object.kind || object.type || 'object')}
          name={String(object.id || object.name || '')}
          description={object.description}
          detail={JSON.stringify({ labels: object.labels, dependencies: object.dependencies, capabilities: object.capabilities }, null, 2)}
        />
      ))}
    </FilteredList>
  );
}

export function McpPanel() {
  const mcp = useStore((s) => s.mcp);
  const servers: any[] = mcp && Array.isArray(mcp.servers) ? mcp.servers : []; // eslint-disable-line @typescript-eslint/no-explicit-any
  const [filter, setFilter] = useState('');
  const q = filter.trim().toLowerCase();
  const tone = (state: string) => (state === 'connected' ? 'success' : state === 'failed' ? 'destructive' : state === 'degraded' ? 'warning' : 'secondary');
  return (
    <FilteredList count={servers.length} label="MCP 服务" filter={filter} setFilter={setFilter}>
      {!servers.length && <EmptyHint icon={PlugIcon} title="未发现 MCP 服务">请在运行时配置 [[mcp_servers]] 后重启服务。</EmptyHint>}
      {servers.filter((s) => includes(q, s.name, s.state, s.transport)).map((server) => (
        <RegistryCard
          key={server.name}
          kind={`MCP · ${server.state || 'unknown'}`}
          kindVariant={tone(String(server.state))}
          name={server.name}
          description={server.error ? String(server.error) : ''}
          meta={server.tool_count !== undefined ? `${server.tool_count} 个工具 · ${server.transport || ''}` : ''}
          detail={server.server_info ? JSON.stringify(server.server_info, null, 2) : ''}
        />
      ))}
    </FilteredList>
  );
}

export function SkillsPanel() {
  const skills = useStore((s) => s.skills);
  const [filter, setFilter] = useState('');
  const q = filter.trim().toLowerCase();
  return (
    <FilteredList
      count={skills.length}
      label="技能库"
      filter={filter}
      setFilter={setFilter}
      actions={<Tip text="重新发现技能"><Button variant="ghost" size="icon-xs" aria-label="刷新技能" onClick={refreshSkills}><RefreshCwIcon /></Button></Tip>}
    >
      {!skills.length && <EmptyHint icon={PuzzleIcon} title="没有技能">运行时会从配置目录加载 SKILL.md。</EmptyHint>}
      {skills.filter((s) => includes(q, s.name, s.id, s.description)).map((skill) => {
        const name = String(skill.name || skill.id || '').replace(/^skill:/, '');
        return (
          <RegistryCard
            key={name}
            kind="skill"
            kindVariant="brand"
            name={name}
            description={skill.description}
            meta={skill.path}
            detail={skill.instructions ? String(skill.instructions).slice(0, 2000) : ''}
          />
        );
      })}
    </FilteredList>
  );
}
