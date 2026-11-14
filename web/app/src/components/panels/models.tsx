// 模型面板：任务类型（路由层级）→ 模型分配。服务端 RoleRouter 在路由决策后按类型改绑模型；
// 空分配 = 跟随自动路由。

import { useCallback, useEffect, useState } from 'react';
import { RotateCcwIcon } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { NativeSelect } from '@/components/ui/native-select';
import { Spinner } from '@/components/ui/spinner';
import { toast } from '@/components/ui/toaster';
import { EmptyHint } from '@/components/panels/shared';
import { api, apiPut } from '@/lib/api';

const typeDescriptions: Record<string, [string, string]> = {
  nano: ['轻量问答', '简单查询与短文本处理'],
  fast: ['快速任务', '翻译、摘要等低复杂度工作'],
  balanced: ['常规对话与规划', '日常任务分析、对话与执行规划'],
  reasoning: ['深度推理', '调试、复杂问题拆解与推理链'],
  frontier: ['复杂架构', '架构设计、证明与重写级任务'],
  specialist: ['编码专精', '以代码为中心的专项任务'],
};

type Tier = { id: string; rank: number; min_complexity: number; max_complexity: number };
type Data = { roles: Record<string, string>; default_model?: string; tiers: Tier[]; models: { id: string; tier?: string }[] };

function rangeText(tier: Tier) {
  const min = Number(tier.min_complexity);
  const max = Number(tier.max_complexity);
  if (min === 255 && max === 255) return '手动层级';
  if (min >= max) return `复杂度 ${min}`;
  return `复杂度 ${min}–${max}`;
}

export function ModelsPanel({ active }: { active: boolean }) {
  const [data, setData] = useState<Data | null>(null);
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    try { setData(await api('/api/model-roles')); setError(''); }
    catch (err) { setError(`加载失败：${(err as Error).message}`); }
  }, []);

  useEffect(() => { if (active) refresh(); }, [active, refresh]);
  useEffect(() => {
    document.addEventListener('workspace-changed', refresh);
    return () => document.removeEventListener('workspace-changed', refresh);
  }, [refresh]);

  const saveRoles = async (roles: Record<string, string>, ok: string) => {
    try {
      const saved = await apiPut('/api/model-roles', { roles });
      setData((d) => (d ? { ...d, roles: saved.roles || roles } : d));
      toast(ok);
    } catch (err) { toast(`保存失败：${(err as Error).message}`, 'error'); }
  };

  if (error) return <EmptyHint title="无法读取模型分配">{error}</EmptyHint>;
  if (!data) return <EmptyHint><Spinner /></EmptyHint>;

  const tiers = [...data.tiers].sort((a, b) => Number(a.rank) - Number(b.rank));
  const byTier = new Map<string, string[]>();
  for (const model of data.models) {
    const tier = String(model.tier || 'other');
    if (!byTier.has(tier)) byTier.set(tier, []);
    byTier.get(tier)!.push(model.id);
  }

  return (
    <div className="grid content-start gap-3 overflow-y-auto p-3">
      <div className="flex items-start gap-3 rounded-lg border bg-card p-3">
        <p className="flex-1 text-xs leading-relaxed text-muted-foreground">
          为不同类型的任务分配模型；发送时未显式选择模型则按任务类型改绑。
          {data.default_model && <> 默认：<code className="font-mono text-foreground">{data.default_model}</code>。</>}
        </p>
        <Button variant="outline" size="xs" disabled={!data.default_model}
          onClick={() => saveRoles(Object.fromEntries(data.tiers.map((t) => [t.id, data.default_model || ''])), '已恢复默认分配')}>
          <RotateCcwIcon />恢复默认
        </Button>
      </div>
      <div className="divide-y rounded-lg border bg-card">
        {tiers.map((tier) => {
          const [title, description] = typeDescriptions[tier.id] || [tier.id, ''];
          const current = String(data.roles[tier.id] ?? '');
          return (
            <div key={tier.id} className="flex items-center gap-3 px-3 py-2.5">
              <div className="grid min-w-0 flex-1 gap-0.5">
                <span className="flex items-center gap-1.5 text-[13px] font-medium">
                  {title}
                  {current
                    ? <Badge variant={current === data.default_model ? 'secondary' : 'success'}>{current === data.default_model ? '默认' : '已指定'}</Badge>
                    : <Badge variant="outline">自动</Badge>}
                </span>
                <span className="truncate text-[11px] text-muted-foreground">{description} · {rangeText(tier)}</span>
              </div>
              <NativeSelect
                size="sm"
                aria-label="该任务类型使用的模型"
                className="w-40 shrink-0 [&_select]:font-mono"
                value={current}
                onChange={(e) => saveRoles({ ...data.roles, [tier.id]: e.target.value }, '模型分配已保存')}
              >
                <option value="">跟随自动路由</option>
                {[...byTier].map(([group, ids]) => (
                  <optgroup key={group} label={group}>
                    {ids.map((id) => <option key={id} value={id}>{id}</option>)}
                  </optgroup>
                ))}
              </NativeSelect>
            </div>
          );
        })}
      </div>
    </div>
  );
}
