import * as React from 'react';
import { ChevronDownIcon } from 'lucide-react';
import { cn } from '@/lib/utils';

// shadcn NativeSelect：原生 <select> 外观对齐 Input。Radix Select 运行期注入
// <style>（违反 CSP style-src 'self'），工作台的下拉选择统一使用原生控件。
function NativeSelect({ className, size = 'default', ...props }: Omit<React.ComponentProps<'select'>, 'size'> & { size?: 'sm' | 'default' }) {
  return (
    <div className={cn('group/native-select relative w-fit has-[select:disabled]:opacity-50', className)} data-slot="native-select-wrapper">
      <select
        data-slot="native-select"
        className={cn(
          'w-full min-w-0 appearance-none rounded-md border border-input bg-transparent pr-8 pl-2.5 text-sm shadow-xs transition-[color,box-shadow] outline-none disabled:pointer-events-none disabled:cursor-not-allowed dark:bg-input/30 [&>option]:bg-popover [&>optgroup]:bg-popover',
          'focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50',
          size === 'sm' ? 'h-8 text-xs' : 'h-9',
        )}
        {...props}
      />
      <ChevronDownIcon className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 text-muted-foreground opacity-60 select-none" aria-hidden="true" />
    </div>
  );
}

export { NativeSelect };
