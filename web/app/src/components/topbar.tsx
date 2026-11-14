import { MenuIcon, MonitorIcon, MoonIcon, PanelRightIcon, Settings2Icon, SunIcon, WifiIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Separator } from '@/components/ui/separator';
import { Tip } from '@/components/ui/tooltip';
import { cycleTheme } from '@/lib/controller';
import { setState, useStore } from '@/lib/store';
import { toast } from '@/components/ui/toaster';
import { cn } from '@/lib/utils';

export function Topbar() {
  const label = useStore((s) => s.workspaceLabel);
  const path = useStore((s) => s.workspacePath);
  const simulation = useStore((s) => s.simulation);
  const online = useStore((s) => s.apiOnline);
  const lanUrls = useStore((s) => s.lanUrls);
  const theme = useStore((s) => s.ui.theme);
  const inspectorOpen = useStore((s) => s.inspectorOpen);
  const sidebarOpen = useStore((s) => s.sidebarOpen);

  const copyLan = () => {
    const value = lanUrls[0];
    if (!navigator.clipboard?.writeText) { toast(value); return; }
    navigator.clipboard.writeText(value).then(() => toast(`已复制 ${value}`)).catch(() => toast(value));
  };

  const ThemeIcon = theme === 'light' ? SunIcon : theme === 'system' ? MonitorIcon : MoonIcon;
  const themeLabel = theme === 'light' ? '浅色' : theme === 'system' ? '跟随系统' : '深色';

  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b bg-sidebar px-3">
      <Button
        id="toggle-sidebar"
        variant="ghost"
        size="icon-sm"
        className="min-[821px]:hidden"
        aria-label="会话导航"
        aria-controls="sidebar"
        aria-expanded={sidebarOpen}
        onClick={() => setState({ sidebarOpen: !sidebarOpen })}
      >
        <MenuIcon />
      </Button>
      <a href="/" className="flex items-center gap-2 rounded-md px-1 text-foreground no-underline" aria-label="Samho 工作台首页">
        <span className="grid size-6 place-items-center rounded-md bg-primary text-xs font-bold text-primary-foreground">S</span>
        <span className="text-sm font-semibold tracking-tight">Samho</span>
      </a>
      <Separator orientation="vertical" className="mx-1 !h-5" />
      <div className="flex min-w-0 items-center gap-2 text-sm" title={path ? `${label} · ${path}` : undefined}>
        <span className="truncate text-muted-foreground">{label || '正在连接工作区…'}</span>
        {simulation && (
          <span className="shrink-0 rounded-md bg-warning/15 px-1.5 py-0.5 text-[11px] font-medium text-warning">模拟模式</span>
        )}
      </div>

      <div className="flex-1" />

      {lanUrls.length > 0 && (
        <Tip text={<span className="whitespace-pre-line">{`局域网访问地址（点击复制）\n${lanUrls.join('\n')}`}</span>}>
          <Button
            id="lan-endpoint"
            variant="outline"
            size="xs"
            className="hidden font-mono text-[11px] font-normal text-muted-foreground xl:inline-flex"
            aria-label="局域网访问地址"
            onClick={copyLan}
          >
            <WifiIcon />{lanUrls[0].replace(/^http:\/\//, '')}
          </Button>
        </Tip>
      )}

      <div className="flex items-center gap-1.5 px-1.5 text-xs text-muted-foreground" role="status">
        <span
          id="connection-dot"
          className={cn(
            'size-2 rounded-full',
            online === null && 'bg-muted-foreground/40',
            online === true && 'bg-success shadow-[0_0_0_3px] shadow-success/20',
            online === false && 'bg-destructive',
          )}
        />
        <span className="hidden sm:inline">{online === null ? '连接中' : online ? '运行时已连接' : '运行时不可达'}</span>
      </div>

      <Tip text={`主题：${themeLabel}`}>
        <Button id="theme-toggle" variant="ghost" size="icon-sm" aria-label="切换主题" onClick={cycleTheme}>
          <ThemeIcon />
        </Button>
      </Tip>
      <Tip text="界面设置">
        <Button id="open-settings" variant="ghost" size="icon-sm" aria-label="界面设置" onClick={() => setState({ settingsOpen: true })}>
          <Settings2Icon />
        </Button>
      </Tip>
      <Tip text={inspectorOpen ? '收起工具面板' : '展开工具面板'}>
        <Button
          id="toggle-dock"
          variant="ghost"
          size="icon-sm"
          aria-label="工具面板"
          aria-pressed={inspectorOpen}
          aria-controls="inspector"
          onClick={() => setState({ inspectorOpen: !inspectorOpen })}
        >
          <PanelRightIcon />
        </Button>
      </Tip>
    </header>
  );
}
