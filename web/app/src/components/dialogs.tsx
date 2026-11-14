// 命令式对话框：promptText / confirmAction 返回 Promise，由 <DialogHost/> 统一渲染。
// 替代各处手写的“重命名 / 删除确认”对话框，保持一致的 shadcn Dialog 外观。

import { useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

type PromptRequest = {
  kind: 'prompt';
  title: string;
  description?: ReactNode;
  label?: string;
  value?: string;
  placeholder?: string;
  maxLength?: number;
  confirmText?: string;
  resolve: (value: string | null) => void;
};

type ConfirmRequest = {
  kind: 'confirm';
  title: string;
  description?: ReactNode;
  confirmText?: string;
  destructive?: boolean;
  resolve: (value: boolean) => void;
};

type Request = PromptRequest | ConfirmRequest;

let current: Request | null = null;
const listeners = new Set<() => void>();
const set = (next: Request | null) => { current = next; listeners.forEach((fn) => fn()); };

export function promptText(options: Omit<PromptRequest, 'kind' | 'resolve'>) {
  return new Promise<string | null>((resolve) => set({ kind: 'prompt', ...options, resolve }));
}

export function confirmAction(options: Omit<ConfirmRequest, 'kind' | 'resolve'>) {
  return new Promise<boolean>((resolve) => set({ kind: 'confirm', ...options, resolve }));
}

export function DialogHost() {
  const request = useSyncExternalStore((fn) => { listeners.add(fn); return () => listeners.delete(fn); }, () => current);
  const [value, setValue] = useState('');
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (request?.kind === 'prompt') {
      setValue(request.value ?? '');
      setInvalid(false);
      requestAnimationFrame(() => inputRef.current?.select());
    }
  }, [request]);

  const close = (result: string | null | boolean) => {
    if (!request) return;
    set(null);
    if (request.kind === 'prompt') request.resolve(typeof result === 'string' ? result : null);
    else request.resolve(result === true);
  };

  const submit = () => {
    if (request?.kind === 'prompt') {
      const text = value.trim();
      if (!text) { setInvalid(true); inputRef.current?.focus(); return; }
      close(text);
    } else close(true);
  };

  return (
    <Dialog open={Boolean(request)} onOpenChange={(open) => { if (!open) close(request?.kind === 'prompt' ? null : false); }}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{request?.title}</DialogTitle>
          {request?.description
            ? <DialogDescription>{request.description}</DialogDescription>
            : <DialogDescription className="sr-only">{request?.title}</DialogDescription>}
        </DialogHeader>
        {request?.kind === 'prompt' && (
          <form className="grid gap-2" onSubmit={(e) => { e.preventDefault(); submit(); }}>
            {request.label && <Label htmlFor="prompt-input">{request.label}</Label>}
            <Input
              id="prompt-input"
              ref={inputRef}
              value={value}
              maxLength={request.maxLength}
              placeholder={request.placeholder}
              aria-invalid={invalid || undefined}
              aria-label={request.label || request.title}
              spellCheck={false}
              onChange={(e) => { setValue(e.target.value); setInvalid(false); }}
            />
          </form>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => close(request?.kind === 'prompt' ? null : false)}>取消</Button>
          <Button
            variant={request?.kind === 'confirm' && request.destructive ? 'destructive' : 'default'}
            onClick={submit}
          >
            {request?.confirmText || (request?.kind === 'prompt' ? '保存' : '确认')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
