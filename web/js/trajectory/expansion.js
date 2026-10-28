// 轨迹展开系统（ZCode ModelTrajectoryExpansion 的移植，42 行核心）：
// 按视觉角色的批量命令携带 version；单项 override 记录当时的 version；
// 命令 version 更新时批量命令覆盖旧 override，用户手点不受后续同版本命令影响。

const ROLE_DEFAULTS = {
  system: false,
  user: true,
  reasoning: false,
  assistant: true,
  'tool-call': false,
  'tool-result': false,
};

export const TRAJECTORY_ROLES = Object.keys(ROLE_DEFAULTS);

const ROLE_LABELS = {
  system: '系统消息',
  user: '用户消息',
  reasoning: '思考过程',
  assistant: '模型输出',
  'tool-call': '工具调用',
  'tool-result': '工具结果',
};

export const roleLabel = (role) => ROLE_LABELS[role] || role;

export function createExpansion() {
  let version = 0;
  const commands = new Map(); // role -> { expanded, version }
  const overrides = new Map(); // key -> { open, version }
  let revealed = new Set();    // 搜索命中强制展开

  function isOpen(key, role) {
    if (revealed.has(key)) return true;
    const override = overrides.get(key);
    const command = commands.get(role);
    if (override && (!command || override.version >= command.version)) return override.open;
    if (command) return command.expanded;
    return ROLE_DEFAULTS[role] ?? false;
  }

  return {
    isOpen,
    toggle(key, role) {
      overrides.set(key, { open: !this.isOpen(key, role), version });
    },
    setRole(role, expanded) {
      version += 1;
      commands.set(role, { expanded, version });
    },
    getRole(role) {
      return commands.get(role)?.expanded ?? ROLE_DEFAULTS[role] ?? false;
    },
    revealAll(keys) {
      revealed = new Set(keys);
    },
    clearReveal() {
      revealed = new Set();
    },
    reset() {
      commands.clear();
      overrides.clear();
      revealed = new Set();
    },
  };
}
