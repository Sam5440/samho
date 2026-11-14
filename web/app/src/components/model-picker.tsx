import { CheckIcon, RouteIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command';
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog';
import { Kbd } from '@/components/ui/kbd';
import { pickModel } from '@/lib/controller';
import { getState, setState, tierNames, useStore, type State } from '@/lib/store';

type Item = { value: string; label: string; hint: string; group: string; disabled?: boolean; simulated?: boolean };

function contextHint(model: State['models'][number]) {
  const windowTokens = Number(model.context_window || 0);
  const parts = [String(model.provider || '')];
  if (windowTokens) parts.push(`${Math.round(windowTokens / 1000)}k 上下文`);
  return parts.filter(Boolean).join(' · ');
}

function modelItems(s: State): Item[] {
  const items: Item[] = [{ value: '__auto__', label: '自动路由', hint: '按任务复杂度与模型能力自动选择层级', group: '路由' }];
  const tiers = s.tiers.length ? s.tiers : Object.keys(tierNames).map((id) => ({ id, name: tierNames[id] }));
  for (const tier of tiers) {
    const tierId = String(tier.id || tier.name || '');
    if (!tierId) continue;
    const label = (tier as { label?: string }).label || tierNames[tierId] || tier.name || tierId;
    const models = s.models.filter((model) => String(model.tier || '').toLowerCase() === tierId);
    if (!models.length) {
      items.push({ value: `tier:${tierId}`, label: `${label} · 层级`, hint: '该层级暂无已配置模型', group: '层级', disabled: true });
      continue;
    }
    for (const model of models) {
      items.push({
        value: `model:${model.id}`,
        label: model.id,
        hint: contextHint(model),
        group: `${label}（${tierId}）`,
        simulated: s.simulation || String(model.provider || '').toLowerCase() === 'demo',
      });
    }
  }
  return items;
}

function isCurrent(request: State['request'], value: string) {
  if (value === '__auto__') return !request.model && !request.tier;
  if (value.startsWith('model:')) return request.model === value.slice(6);
  if (value.startsWith('tier:')) return !request.model && request.tier === value.slice(5);
  return false;
}

export function ModelPicker() {
  const open = useStore((s) => s.modelPickerOpen);
  const request = useStore((s) => s.request);
  useStore((s) => s.models);
  const items = open ? modelItems(getState()) : [];
  const groups = [...new Set(items.map((i) => i.group))];

  const close = () => {
    setState({ modelPickerOpen: false });
    document.dispatchEvent(new Event('composer-focus'));
  };

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? setState({ modelPickerOpen: true }) : close())}>
      <DialogContent className="overflow-hidden p-0 sm:max-w-lg" showCloseButton={false}>
        <DialogTitle className="sr-only">选择分析与规划模型</DialogTitle>
        <DialogDescription className="sr-only">搜索模型或层级，回车确认</DialogDescription>
        <Command className="[&_[cmdk-item]]:py-2">
          <CommandInput placeholder="搜索模型或层级…" />
          <CommandList className="max-h-[min(26rem,60vh)]">
            <CommandEmpty>没有匹配项</CommandEmpty>
            {groups.map((group) => (
              <CommandGroup key={group} heading={group}>
                {items.filter((i) => i.group === group).map((item) => (
                  <CommandItem
                    key={item.value}
                    value={`${item.label} ${item.hint} ${item.group}`}
                    disabled={item.disabled}
                    onSelect={() => { pickModel(item.value); close(); }}
                  >
                    {item.value === '__auto__' ? <RouteIcon className="text-brand" /> : <span className="size-4" />}
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-mono text-[13px]">{item.label}</div>
                      {item.hint && <div className="truncate text-xs text-muted-foreground">{item.hint}</div>}
                    </div>
                    {item.value.startsWith('model:') && (
                      item.simulated ? <Badge variant="warning">模拟</Badge> : <Badge variant="success">真实</Badge>
                    )}
                    {isCurrent(request, item.value) && <CheckIcon className="text-brand" />}
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
          <div className="flex items-center gap-3 border-t px-3 py-2 text-xs text-muted-foreground">
            <span className="flex items-center gap-1"><Kbd>↵</Kbd>选择</span>
            <span className="flex items-center gap-1"><Kbd>↑</Kbd><Kbd>↓</Kbd>导航</span>
            <span className="flex items-center gap-1"><Kbd>Ctrl</Kbd><Kbd>M</Kbd>打开</span>
            <span className="ml-auto flex items-center gap-1"><Kbd>Esc</Kbd>关闭</span>
          </div>
        </Command>
      </DialogContent>
    </Dialog>
  );
}
