// 面板通用骨架：工具栏 + 空态。

import type { ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

export function PanelToolbar({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('flex h-10 shrink-0 items-center gap-1 border-b px-2', className)}>{children}</div>;
}

export function EmptyHint({ icon: Icon, title, children, className }: {
  icon?: LucideIcon; title?: ReactNode; children?: ReactNode; className?: string;
}) {
  return (
    <div className={cn('flex flex-col items-center justify-center gap-1.5 px-6 py-10 text-center', className)}>
      {Icon && <Icon className="mb-1 size-5 text-muted-foreground/60" />}
      {title && <div className="text-sm font-medium text-foreground/80">{title}</div>}
      {children && <div className="text-xs leading-relaxed text-muted-foreground">{children}</div>}
    </div>
  );
}

export function SectionLabel({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground', className)}>{children}</div>;
}
