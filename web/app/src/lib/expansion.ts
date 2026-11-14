// 轨迹展开系统：按视觉角色的批量命令携带 version；单项 override 记录当时的 version；
// 命令 version 更新时批量命令覆盖旧 override，用户手点不受后续同版本命令影响。

const ROLE_DEFAULTS: Record<string, boolean> = {
  system: false,
  user: true,
  reasoning: false,
  assistant: true,
  'tool-call': false,
  'tool-result': false,
};

export const TRAJECTORY_ROLES = Object.keys(ROLE_DEFAULTS);

const ROLE_LABELS: Record<string, string> = {
  system: '系统消息',
  user: '用户消息',
  reasoning: '思考过程',
  assistant: '模型输出',
  'tool-call': '工具调用',
  'tool-result': '工具结果',
};

export const roleLabel = (role: string) => ROLE_LABELS[role] || role;

export type Expansion = ReturnType<typeof createExpansion>;

export function createExpansion() {
  let version = 0;
  const commands = new Map<string, { expanded: boolean; version: number }>();
  const overrides = new Map<string, { open: boolean; version: number }>();

  function isOpen(key: string, role: string) {
    const override = overrides.get(key);
    const command = commands.get(role);
    if (override && (!command || override.version >= command.version)) return override.open;
    if (command) return command.expanded;
    return ROLE_DEFAULTS[role] ?? false;
  }

  return {
    isOpen,
    toggle(key: string, role: string) {
      overrides.set(key, { open: !isOpen(key, role), version });
    },
    setRole(role: string, expanded: boolean) {
      version += 1;
      commands.set(role, { expanded, version });
    },
    getRole(role: string) {
      return commands.get(role)?.expanded ?? ROLE_DEFAULTS[role] ?? false;
    },
  };
}
