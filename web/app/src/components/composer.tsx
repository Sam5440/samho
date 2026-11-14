// 输入区：自适应 textarea（Enter 直发 / Shift+Enter 换行）+ 自动分级/复杂度 + 技能多选
// + 模型选择 + 运行中控制（调整方向 / 追加任务 / 停止）。

import { useEffect, useRef, useState } from 'react';
import { ArrowUpIcon, ChevronDownIcon, CornerDownRightIcon, CpuIcon, GaugeIcon, PuzzleIcon, SquareIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Slider } from '@/components/ui/slider';
import { Switch } from '@/components/ui/switch';
import { Spinner } from '@/components/ui/spinner';
import { Tip } from '@/components/ui/tooltip';
import { toast } from '@/components/ui/toaster';
import { NativeSelect } from '@/components/ui/native-select';
import { cancelTask, controlTask, submitPrompt } from '@/lib/controller';
import { matchesShortcut } from '@/lib/shortcuts';
import { getState, requestSummary, setState, useStore } from '@/lib/store';
import { cn } from '@/lib/utils';

function RoutingPopover() {
  const request = useStore((s) => s.request);
  const setRequest = (patch: Partial<typeof request>) => setState((s) => ({ request: { ...s.request, ...patch } }));
  return (
    <Popover>
      <Tip text="让路由器从任务描述推断所需智能层级，或手动设定复杂度">
        <PopoverTrigger asChild>
          <Button variant="ghost" size="xs" className="text-muted-foreground" aria-label="路由与复杂度">
            <GaugeIcon />
            <span className="hidden sm:inline">{request.automatic ? '自动分级' : `复杂度 ${request.complexity}`}</span>
          </Button>
        </PopoverTrigger>
      </Tip>
      <PopoverContent align="start" className="w-72">
        <div className="grid gap-4">
          <div className="flex items-center justify-between gap-3">
            <div className="grid gap-0.5">
              <label htmlFor="automatic-toggle" className="text-sm font-medium">自动分级</label>
              <p className="text-xs text-muted-foreground">按任务描述的关键词推断复杂度</p>
            </div>
            <Switch id="automatic-toggle" checked={request.automatic} onCheckedChange={(checked) => setRequest({ automatic: checked })} />
          </div>
          <div className="grid gap-2">
            <div className="flex items-center justify-between text-sm">
              <span className={cn('font-medium', request.automatic && 'text-muted-foreground')}>任务复杂度</span>
              <output htmlFor="complexity-range" className="font-mono text-sm tabular-nums">{request.complexity}</output>
            </div>
            <Slider
              id="complexity-range"
              aria-label="任务复杂度"
              min={0}
              max={10}
              step={1}
              value={[request.complexity]}
              onValueChange={([value]) => setRequest({ complexity: value, automatic: false })}
            />
            <div className="flex justify-between text-[10px] text-muted-foreground"><span>轻量</span><span>均衡</span><span>前沿</span></div>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function SkillsPopover() {
  const skills = useStore((s) => s.skills);
  const selected = useStore((s) => s.selectedSkills);
  const toggle = (name: string, on: boolean) => setState((s) => ({
    selectedSkills: on ? [...new Set([...s.selectedSkills, name])] : s.selectedSkills.filter((x) => x !== name),
  }));
  return (
    <Popover>
      <Tip text="显式加载技能（SKILL.md）">
        <PopoverTrigger asChild>
          <Button variant="ghost" size="xs" className={cn('text-muted-foreground', selected.length && 'text-brand')} aria-label="选择技能">
            <PuzzleIcon />
            <span className="hidden sm:inline">技能</span>
            {selected.length > 0 && <span className="rounded bg-brand/15 px-1 font-mono text-[10px] text-brand">{selected.length}</span>}
          </Button>
        </PopoverTrigger>
      </Tip>
      <PopoverContent align="start" className="w-80 p-1.5">
        <div className="px-2 pt-1 pb-1.5 text-xs font-medium text-muted-foreground">技能</div>
        <div className="max-h-72 overflow-y-auto">
          {!skills.length && <p className="px-2 py-3 text-sm text-muted-foreground">尚未发现技能（SKILL.md）。</p>}
          {skills.map((skill) => {
            const name = String(skill.name || skill.id || '').replace(/^skill:/, '');
            if (!name) return null;
            const checked = selected.includes(name);
            return (
              <label key={name} htmlFor={`skill-${name}`} className="flex cursor-pointer items-start gap-2.5 rounded-md px-2 py-1.5 hover:bg-accent">
                <Checkbox id={`skill-${name}`} checked={checked} onCheckedChange={(v) => toggle(name, v === true)} className="mt-0.5" />
                <span className="grid min-w-0">
                  <span className="truncate text-sm font-medium">{name}</span>
                  {skill.description && <span className="line-clamp-2 text-xs text-muted-foreground">{String(skill.description)}</span>}
                </span>
              </label>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}

export function Composer() {
  const task = useStore((s) => s.task);
  const request = useStore((s) => s.request);
  const submitKey = useStore((s) => s.ui.submit_key);
  const running = Boolean(task);
  const [value, setValue] = useState('');
  const [mode, setMode] = useState<'steer' | 'follow-up'>('steer');
  const [sendingControl, setSendingControl] = useState(false);
  // 提交进行中标记：覆盖 /api/run 尚未返回、running 未置位的窗口期。
  const submitInFlight = useRef(false);
  const [inFlight, setInFlight] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const cancelRequested = Boolean(task?.cancelRequested);

  const autoResize = () => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 240)}px`;
  };

  useEffect(autoResize, [value]);

  useEffect(() => {
    const focus = () => textareaRef.current?.focus();
    const fill = (event: Event) => {
      setValue(String((event as CustomEvent).detail || ''));
      requestAnimationFrame(focus);
    };
    document.addEventListener('composer-focus', focus);
    document.addEventListener('composer-fill', fill);
    focus();
    return () => {
      document.removeEventListener('composer-focus', focus);
      document.removeEventListener('composer-fill', fill);
    };
  }, []);

  useEffect(() => { if (!running) setSendingControl(false); }, [running]);

  async function submitFlow() {
    const textarea = textareaRef.current;
    const prompt = value.trim();
    if (!prompt) return;
    if (running) {
      if (!getState().task?.id || sendingControl || cancelRequested) return;
      setSendingControl(true);
      try {
        const queued = await controlTask(prompt, mode);
        toast(`${mode === 'steer' ? '已排队调整方向' : '已排队追加任务'} · 队列序号 ${queued?.sequence ?? ''}`);
        setValue('');
      } catch (error) {
        toast(`任务调整失败：${(error as Error).message}`, 'error');
      } finally {
        setSendingControl(false);
      }
      return;
    }
    // 提交锁：/api/run 返回前忽略再次提交，避免慢响应期间连发启动多个未受追踪的任务。
    if (submitInFlight.current) return;
    submitInFlight.current = true;
    setInFlight(true);
    const original = value;
    setValue('');
    try {
      const failure = await submitPrompt(original);
      // 失败回填只在输入框仍为空时进行，不覆盖用户新输入。
      if (failure && textarea && !textarea.value.trim()) setValue(original);
    } finally {
      submitInFlight.current = false;
      setInFlight(false);
    }
  }

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (matchesShortcut(event.nativeEvent, submitKey) || (event.key === 'Enter' && !event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey)) {
      event.preventDefault();
      submitFlow();
    }
  };

  const disabled = !value.trim() || inFlight || (running && (!task?.id || sendingControl || cancelRequested));

  return (
    <div className="shrink-0 px-4 pb-4">
      <form
        className="mx-auto max-w-3xl"
        onSubmit={(event) => { event.preventDefault(); submitFlow(); }}
      >
        <div className={cn(
          'rounded-2xl border bg-card shadow-sm transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/20',
          running && 'border-brand/40',
        )}>
          {running && (
            <div className="flex items-center gap-2 border-b px-3 py-1.5 text-xs text-muted-foreground">
              <CornerDownRightIcon className="size-3.5" />
              <span>任务运行中，发送内容将</span>
              <NativeSelect
                size="sm"
                aria-label="运行中发送模式"
                title="任务运行中发送内容的方式"
                value={mode}
                onChange={(e) => setMode(e.target.value as 'steer' | 'follow-up')}
                className="[&_select]:h-6 [&_select]:border-0 [&_select]:bg-transparent [&_select]:pl-1 [&_select]:text-xs [&_select]:font-medium [&_select]:text-foreground [&_select]:shadow-none"
              >
                <option value="steer">调整方向 · 下一轮生效</option>
                <option value="follow-up">追加任务 · 完成后处理</option>
              </NativeSelect>
            </div>
          )}
          <textarea
            ref={textareaRef}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={onKeyDown}
            rows={2}
            maxLength={100000}
            aria-label="任务内容"
            placeholder={running ? '补充指令…' : '描述你的任务…'}
            className="composer-input block max-h-60 min-h-14 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-sm leading-relaxed outline-none placeholder:text-muted-foreground"
          />
          <div className="flex items-center gap-1 px-2 pb-2">
            <RoutingPopover />
            <SkillsPopover />
            <Tip text="当前任务分析与规划模型（Ctrl+M）">
              <Button
                variant="ghost"
                size="xs"
                aria-haspopup="dialog"
                className="model-chip max-w-[14rem] text-muted-foreground"
                onClick={() => setState({ modelPickerOpen: true })}
              >
                <CpuIcon />
                <span className="truncate font-mono text-[11.5px]">{requestSummary(request)}</span>
                <ChevronDownIcon className="opacity-60" />
              </Button>
            </Tip>
            <div className="flex-1" />
            <span className="mr-1 hidden text-[11px] text-muted-foreground md:inline">Enter 发送 · Shift+Enter 换行</span>
            {running && (
              <Button variant="outline" size="xs" onClick={() => cancelTask()} disabled={cancelRequested} className="composer-cancel">
                {cancelRequested ? <Spinner className="size-3" /> : <SquareIcon className="fill-current" />}
                {cancelRequested ? '正在取消…' : '停止'}
              </Button>
            )}
            <Tip text={running ? (mode === 'follow-up' ? '追加任务' : '调整方向') : '发送'}>
              <Button
                type="submit"
                size="icon-sm"
                variant={running ? 'secondary' : 'brand'}
                className="composer-send rounded-full"
                disabled={disabled}
                aria-label={running ? (mode === 'follow-up' ? '追加任务' : '调整方向') : '发送'}
              >
                {inFlight || sendingControl ? <Spinner /> : <ArrowUpIcon />}
              </Button>
            </Tip>
          </div>
        </div>
      </form>
    </div>
  );
}
