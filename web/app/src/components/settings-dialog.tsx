import { useEffect, useState } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { NativeSelect } from '@/components/ui/native-select';
import { toast } from '@/components/ui/toaster';
import { saveUiSettings } from '@/lib/controller';
import { setState, useStore, type UiSettings } from '@/lib/store';

export function SettingsDialog() {
  const open = useStore((s) => s.settingsOpen);
  const ui = useStore((s) => s.ui);
  const [draft, setDraft] = useState<UiSettings>(ui);
  const [saving, setSaving] = useState(false);

  useEffect(() => { if (open) setDraft(ui); }, [open, ui]);

  const field = (key: keyof UiSettings, label: string) => (
    <div className="grid gap-2">
      <Label htmlFor={`ui-${key}`}>{label}</Label>
      <Input
        id={`ui-${key}`}
        value={draft[key]}
        maxLength={40}
        className="font-mono"
        onChange={(e) => setDraft({ ...draft, [key]: e.target.value })}
      />
    </div>
  );

  const save = async () => {
    const value = {
      theme: draft.theme,
      submit_key: draft.submit_key.trim(),
      new_session_key: draft.new_session_key.trim(),
      cancel_key: draft.cancel_key.trim(),
    };
    if (!value.submit_key || !value.new_session_key || !value.cancel_key) {
      toast('快捷键不能为空', 'error');
      return;
    }
    setSaving(true);
    try {
      await saveUiSettings(value);
      setState({ settingsOpen: false });
      toast('设置已保存到当前工作区');
    } catch (error) {
      toast(`保存失败：${(error as Error).message}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => setState({ settingsOpen: next })}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>界面与快捷键</DialogTitle>
          <DialogDescription>保存到当前工作区（ui.json）。快捷键格式如 Ctrl+Enter、Alt+N。</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="ui-theme">主题</Label>
            <NativeSelect
              id="ui-theme"
              className="w-full"
              value={draft.theme}
              onChange={(e) => setDraft({ ...draft, theme: e.target.value as UiSettings['theme'] })}
            >
              <option value="dark">深色</option>
              <option value="light">浅色</option>
              <option value="system">跟随系统</option>
            </NativeSelect>
          </div>
          {field('submit_key', '发送任务')}
          {field('new_session_key', '新建会话')}
          {field('cancel_key', '取消任务')}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setState({ settingsOpen: false })}>取消</Button>
          <Button onClick={save} disabled={saving}>保存</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
