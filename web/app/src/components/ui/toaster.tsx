import { useSyncExternalStore } from 'react';
import { CircleAlertIcon, CircleCheckIcon, XIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

// 轻量 Toaster（shadcn sonner 外观）。sonner 运行期注入 <style>，与 CSP
// style-src 'self' 冲突，这里用同等视觉的最小实现。
type ToastItem = { id: number; message: string; variant: 'default' | 'error' | 'success' };

let items: ToastItem[] = [];
let seq = 0;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((fn) => fn());

export function toast(message: string, variant: ToastItem['variant'] = 'default') {
  const item = { id: ++seq, message: String(message), variant };
  items = [...items.slice(-4), item];
  emit();
  setTimeout(() => dismiss(item.id), variant === 'error' ? 8000 : 4000);
}

function dismiss(id: number) {
  items = items.filter((t) => t.id !== id);
  emit();
}

export function Toaster() {
  const list = useSyncExternalStore(
    (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
    () => items,
  );
  return (
    <div aria-live="polite" className="pointer-events-none fixed right-4 bottom-4 z-[100] flex w-[min(22rem,calc(100vw-2rem))] flex-col gap-2">
      {list.map((t) => (
        <div
          key={t.id}
          role="status"
          className={cn(
            'pointer-events-auto flex items-start gap-2.5 rounded-lg border bg-popover px-3.5 py-3 text-sm text-popover-foreground shadow-lg animate-in fade-in-0 slide-in-from-bottom-2',
            t.variant === 'error' && 'border-destructive/40',
          )}
        >
          {t.variant === 'error'
            ? <CircleAlertIcon className="mt-0.5 size-4 shrink-0 text-destructive" />
            : <CircleCheckIcon className="mt-0.5 size-4 shrink-0 text-success" />}
          <span className="min-w-0 flex-1 break-words">{t.message}</span>
          <button type="button" aria-label="关闭通知" className="text-muted-foreground hover:text-foreground" onClick={() => dismiss(t.id)}>
            <XIcon className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}
