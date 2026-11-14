// 设备面板的命令式句柄（终端/浏览器挂载后注册）：侧栏附属行与会话切换通过它
// 打开/恢复会话绑定的面板，并读取各会话当前绑定数量。

export type AttachHandle = {
  show(options: { sessionId?: string | null }): Promise<void>;
  open(sessionId: string): Promise<void>;
  counts(sessionId: string): number;
};

export const bridge: { terminal: AttachHandle | null; browser: AttachHandle | null } = {
  terminal: null,
  browser: null,
};
