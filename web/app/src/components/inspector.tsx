// 右栏停靠：设备组（文件/Git/终端/浏览器）+ 检查组（轨迹/子代理/任务/对象/MCP/技能/模型）。
// 全部面板常驻挂载（终端 PTY、浏览器标签、文件选择等状态跨切换保留），以 active 控制可见与轮询。

import type { ReactNode } from 'react';
import {
  ActivityIcon, BlocksIcon, BotIcon, CpuIcon, FolderIcon, GitBranchIcon, GlobeIcon, KanbanSquareIcon,
  PlugIcon, PuzzleIcon, SquareTerminalIcon, type LucideIcon,
} from 'lucide-react';
import { Tip } from '@/components/ui/tooltip';
import { FilesPanel } from '@/components/panels/files';
import { GitPanel } from '@/components/panels/git';
import { TerminalPanel } from '@/components/panels/terminal';
import { BrowserPanel } from '@/components/panels/browser';
import { TrajectoryPanel } from '@/components/panels/trajectory';
import { AgentsPanel } from '@/components/panels/agents';
import { TasksPanel } from '@/components/panels/tasks';
import { McpPanel, ObjectsPanel, SkillsPanel } from '@/components/panels/registry';
import { ModelsPanel } from '@/components/panels/models';
import { activatePanel } from '@/lib/controller';
import { DEVICE_TABS, INSPECT_TABS, useStore, type PanelName } from '@/lib/store';
import { cn } from '@/lib/utils';

const META: Record<PanelName, { label: string; icon: LucideIcon; aria: string }> = {
  files: { label: '文件', icon: FolderIcon, aria: '文件管理' },
  git: { label: 'Git', icon: GitBranchIcon, aria: 'Git 审查' },
  term: { label: '终端', icon: SquareTerminalIcon, aria: '终端' },
  browser: { label: '浏览器', icon: GlobeIcon, aria: '浏览器' },
  trajectory: { label: '轨迹', icon: ActivityIcon, aria: '模型轨迹' },
  agents: { label: '子代理', icon: BotIcon, aria: '子代理' },
  tasks: { label: '任务', icon: KanbanSquareIcon, aria: '任务看板' },
  objects: { label: '对象', icon: BlocksIcon, aria: '对象注册表' },
  mcp: { label: 'MCP', icon: PlugIcon, aria: 'MCP 服务' },
  skills: { label: '技能', icon: PuzzleIcon, aria: '技能库' },
  models: { label: '模型', icon: CpuIcon, aria: '模型分配' },
};

// compact：未选中的标签只显示图标（检查组 7 项，右栏宽度放不下全部文字）。
function GroupTabs({ group, tabs, compact }: { group: string; tabs: PanelName[]; compact?: boolean }) {
  const active = useStore((s) => s.activePanel);
  return (
    <div role="tablist" aria-label={group} className="flex min-w-0 items-center gap-0.5">
      {tabs.map((name) => {
        const { label, icon: Icon } = META[name];
        const selected = active === name;
        const button = (
          <button
            key={name}
            type="button"
            role="tab"
            data-panel={name}
            aria-selected={selected}
            aria-controls={`panel-${name}`}
            onClick={() => activatePanel(name)}
            className={cn(
              'flex h-7 items-center gap-1.5 rounded-md px-2 text-xs font-medium whitespace-nowrap text-muted-foreground transition-colors outline-none hover:bg-sidebar-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/50',
              selected && 'bg-background text-foreground shadow-xs ring-1 ring-border dark:bg-sidebar-accent',
            )}
          >
            <Icon className="size-3.5" />
            <span className={cn(compact && !selected && 'sr-only')}>{label}</span>
          </button>
        );
        return compact && !selected ? <Tip key={name} text={label}>{button}</Tip> : button;
      })}
    </div>
  );
}

function PanelSlot({ name, children }: { name: PanelName; children: ReactNode }) {
  const active = useStore((s) => s.activePanel === name);
  return (
    <section
      id={`panel-${name}`}
      role="tabpanel"
      aria-label={META[name].aria}
      hidden={!active}
      className="absolute inset-0 flex flex-col"
    >
      {children}
    </section>
  );
}

export function Inspector() {
  const active = useStore((s) => s.activePanel);
  const streamStatus = useStore((s) => s.streamStatus);
  const is = (name: PanelName) => active === name;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div id="dock-head" className="grid gap-1 border-b px-2 py-2">
        <div className="flex items-center gap-2">
          <span className="w-7 shrink-0 text-[10px] font-medium text-muted-foreground">设备</span>
          <GroupTabs group="设备工具" tabs={DEVICE_TABS} />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-7 shrink-0 text-[10px] font-medium text-muted-foreground">检查</span>
          <div className="min-w-0 overflow-x-auto [scrollbar-width:none]">
            <GroupTabs group="运行时检查" tabs={INSPECT_TABS} compact />
          </div>
        </div>
      </div>
      <div className="relative min-h-0 flex-1">
        <PanelSlot name="files"><FilesPanel active={is('files')} /></PanelSlot>
        <PanelSlot name="git"><GitPanel active={is('git')} /></PanelSlot>
        <PanelSlot name="term"><TerminalPanel active={is('term')} /></PanelSlot>
        <PanelSlot name="browser"><BrowserPanel active={is('browser')} /></PanelSlot>
        <PanelSlot name="trajectory"><TrajectoryPanel /></PanelSlot>
        <PanelSlot name="agents"><AgentsPanel active={is('agents')} /></PanelSlot>
        <PanelSlot name="tasks"><TasksPanel active={is('tasks')} /></PanelSlot>
        <PanelSlot name="objects"><ObjectsPanel /></PanelSlot>
        <PanelSlot name="mcp"><McpPanel /></PanelSlot>
        <PanelSlot name="skills"><SkillsPanel /></PanelSlot>
        <PanelSlot name="models"><ModelsPanel active={is('models')} /></PanelSlot>
      </div>
      <div id="stream-status" className="flex h-7 shrink-0 items-center gap-1.5 border-t px-3 text-[11px] text-muted-foreground">
        <span className={cn('size-1.5 rounded-full', streamStatus.includes('已连接') ? 'bg-success' : 'bg-warning')} />
        {streamStatus}
      </div>
    </div>
  );
}
