import { useEffect, useRef } from 'react';
import { TooltipProvider } from '@/components/ui/tooltip';
import { Toaster } from '@/components/ui/toaster';
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet';
import { DialogHost } from '@/components/dialogs';
import { PreviewHost } from '@/components/preview-dialog';
import { Topbar } from '@/components/topbar';
import { Sidebar } from '@/components/sidebar';
import { Conversation } from '@/components/conversation';
import { Inspector } from '@/components/inspector';
import { SettingsDialog } from '@/components/settings-dialog';
import { ModelPicker } from '@/components/model-picker';
import { useMediaQuery } from '@/lib/hooks';
import { bootstrap, cancelTask, newSession } from '@/lib/controller';
import { getState, setState, useStore } from '@/lib/store';
import { matchesShortcut } from '@/lib/shortcuts';
import { cn } from '@/lib/utils';

export function App() {
  const wide = useMediaQuery('(min-width: 1181px)');
  const desktop = useMediaQuery('(min-width: 821px)');
  const inspectorOpen = useStore((s) => s.inspectorOpen);
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const announcement = useStore((s) => s.announcement);

  const wideRef = useRef(wide);
  wideRef.current = wide;

  useEffect(() => { bootstrap(); }, []);

  // 窄屏默认收起检查器抽屉；回到宽屏恢复常驻。
  useEffect(() => { setState({ inspectorOpen: wide }); }, [wide]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const { ui } = getState();
      if (event.key === 'Escape' && !wideRef.current && getState().inspectorOpen && !document.querySelector('[role="dialog"]')) {
        setState({ inspectorOpen: false });
        return;
      }
      if (matchesShortcut(event, ui.new_session_key)) { event.preventDefault(); newSession(); }
      else if (matchesShortcut(event, ui.cancel_key)) { event.preventDefault(); cancelTask(); }
      else if (matchesShortcut(event, 'Ctrl+M')) { event.preventDefault(); setState({ modelPickerOpen: true }); }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  return (
    <TooltipProvider>
      <div className="flex h-full flex-col">
        <Topbar />
        <div className="flex min-h-0 flex-1">
          {desktop ? (
            <aside id="sidebar" aria-label="会话导航" className="flex w-[17.5rem] shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground">
              <Sidebar />
            </aside>
          ) : (
            <Sheet open={sidebarOpen} onOpenChange={(open) => setState({ sidebarOpen: open })}>
              <SheetContent side="left" id="sidebar" aria-label="会话导航" className="bg-sidebar p-0 pt-10">
                <SheetTitle className="sr-only">会话导航</SheetTitle>
                <SheetDescription className="sr-only">工作区、项目与会话</SheetDescription>
                <Sidebar />
              </SheetContent>
            </Sheet>
          )}
          <main className="relative flex min-w-0 flex-1 flex-col bg-background">
            <Conversation />
          </main>
          {/* 检查器始终挂载（终端 PTY / 浏览器标签需跨收起与断点保留），只切换样式：
              宽屏常驻右栏，窄屏为右侧抽屉。 */}
          {!wide && inspectorOpen && (
            <div aria-hidden="true" className="fixed inset-0 top-12 z-40 bg-black/50 animate-in fade-in-0" onClick={() => setState({ inspectorOpen: false })} />
          )}
          <aside
            id="inspector"
            aria-label="设备与运行时面板"
            hidden={!inspectorOpen}
            className={cn(
              'flex shrink-0 flex-col border-l bg-sidebar',
              wide ? 'w-[25rem]' : 'fixed top-12 right-0 bottom-0 z-50 w-[min(30rem,94vw)] shadow-2xl animate-in slide-in-from-right',
            )}
          >
            <Inspector />
          </aside>
        </div>
      </div>
      <div className="sr-only" role="status" aria-live="polite">{announcement}</div>
      <SettingsDialog />
      <ModelPicker />
      <DialogHost />
      <PreviewHost />
      <Toaster />
    </TooltipProvider>
  );
}
