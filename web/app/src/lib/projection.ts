// 时间线 / 轨迹投影：会话消息 + 实时工具状态 → 行模型；轨迹事件 → 调用卡模型。
// 纯函数、无 DOM 依赖。

/* eslint-disable @typescript-eslint/no-explicit-any */
export type Message = {
  role?: string;
  content?: any;
  reasoning_content?: string;
  reasoning_ms?: number;
  tool_calls?: any[];
  tool_call_id?: string;
  name?: string;
  model_id?: string;
  timestamp_ms?: number;
};

export type ToolStatus = 'running' | 'completed' | 'error';

export type LiveTool = {
  name: string;
  args: Record<string, unknown>;
  input: string;
  status: 'running' | 'done' | 'error';
  result: string;
  isError: boolean;
  latencyMs?: number;
};

export type Row =
  | { type: 'turn'; key: string; turn: number }
  | { type: 'user'; key: string; text: string }
  | { type: 'reasoning'; key: string; text: string; completed: boolean; durationMs?: number }
  | { type: 'text'; key: string; text: string; model: string | null }
  | { type: 'tool'; key: string; callId: string; name: string; args: Record<string, unknown>; input: string; status: ToolStatus; result: string; latencyMs?: number }
  | { type: 'note'; key: string; tone: 'error' | 'muted'; text: string };

export type ToolRow = Extract<Row, { type: 'tool' }>;

export type LiveSnapshot = {
  running: boolean;
  turn?: number;
  attempt?: number;
  modelId?: string;
  reasoning: string;
  text: string;
  tools: Map<string, LiveTool>;
  toolCalls: any[];
  startedAt: number;
};

function parseArgs(raw: unknown): Record<string, unknown> {
  if (!raw) return {};
  if (typeof raw === 'object') return raw as Record<string, unknown>;
  try { return JSON.parse(String(raw)); } catch { return { _raw: String(raw) }; }
}

function resultIsError(content: unknown) {
  if (typeof content !== 'string') return false;
  return /"(?:isError|is_error)"\s*:\s*true/i.test(content) || /^\s*\{\s*"error"/.test(content);
}

export function resultText(content: unknown) {
  if (content === undefined || content === null) return '';
  if (typeof content === 'string') return content;
  try { return JSON.stringify(content, null, 2); } catch { return String(content); }
}

const textOf = (content: unknown) =>
  typeof content === 'string' ? content : JSON.stringify(content, null, 2);

export function projectTimeline(messages: Message[], live: LiveSnapshot | null = null): Row[] {
  const rows: Row[] = [];
  const openTools = new Map<string, ToolRow>();
  let turn = 0;

  messages.forEach((message, index) => {
    const role = String(message.role || '').toLowerCase();
    if (role === 'system') return;

    if (role === 'user') {
      turn += 1;
      rows.push({ type: 'turn', key: `turn:${index}`, turn });
      rows.push({ type: 'user', key: `u:${index}`, text: textOf(message.content) });
      return;
    }

    if (role === 'tool') {
      const row = openTools.get(String(message.tool_call_id));
      if (row) {
        row.result = resultText(message.content);
        row.status = resultIsError(message.content) ? 'error' : 'completed';
      } else {
        rows.push({
          type: 'tool', key: `t:${index}`, name: message.name || 'tool', callId: message.tool_call_id || '',
          args: {}, status: resultIsError(message.content) ? 'error' : 'completed',
          result: resultText(message.content), input: '',
        });
      }
      return;
    }

    if (role === 'assistant') {
      const reasoning = typeof message.reasoning_content === 'string' ? message.reasoning_content : '';
      if (reasoning.trim()) {
        rows.push({ type: 'reasoning', key: `a:${index}:reasoning`, text: reasoning, completed: true, durationMs: message.reasoning_ms });
      }
      const text = typeof message.content === 'string' ? message.content
        : Array.isArray(message.content) ? message.content.map((p: any) => p?.text || '').join('\n') : '';
      if (text.trim()) {
        rows.push({ type: 'text', key: `a:${index}:text`, text, model: message.model_id || null });
      }
      const calls = Array.isArray(message.tool_calls) ? message.tool_calls : [];
      for (const call of calls) {
        const id = String(call?.id || `call:${index}:${rows.length}`);
        const row: ToolRow = {
          type: 'tool', key: `t:${id}`, callId: id,
          name: call?.function?.name || 'tool',
          args: parseArgs(call?.function?.arguments),
          input: typeof call?.function?.arguments === 'string' ? call.function.arguments : JSON.stringify(call?.function?.arguments ?? {}, null, 2),
          status: 'running', result: '',
        };
        openTools.set(id, row);
        rows.push(row);
      }
      return;
    }

    rows.push({ type: 'note', key: `n:${index}`, tone: role === 'error' ? 'error' : 'muted', text: textOf(message.content) });
  });

  // 未配对 tool 消息的调用保持 running（会话被中断）；live 快照可补最终状态。
  if (live) {
    for (const row of rows) {
      if (row.type !== 'tool' || row.status !== 'running') continue;
      const snapshot = live.tools.get(row.callId);
      if (!snapshot) continue;
      row.args = snapshot.args ?? row.args;
      row.input = snapshot.input ?? row.input;
      if (snapshot.status === 'done') { row.status = 'completed'; row.result = snapshot.result || ''; }
      else if (snapshot.status === 'error') { row.status = 'error'; row.result = snapshot.result || ''; }
      if (snapshot.latencyMs) row.latencyMs = snapshot.latencyMs;
    }
  }
  return rows;
}

// 实时工具事件（SSE tool.start / tool.result）→ callId 索引的工具状态。
export function applyLiveToolEvent(tools: Map<string, LiveTool>, event: any) {
  const payload = event.payload || {};
  const callId = String(payload.call_id || '');
  if (!callId) return;
  if (event.kind === 'tool.start') {
    tools.set(callId, {
      name: payload.name || 'tool',
      args: payload.arguments && typeof payload.arguments === 'object' ? payload.arguments : {},
      input: typeof payload.arguments === 'string' ? payload.arguments : JSON.stringify(payload.arguments ?? {}, null, 2),
      status: 'running', result: '', isError: false,
    });
  } else if (event.kind === 'tool.result') {
    const previous = tools.get(callId) || { name: payload.name || 'tool', args: {}, input: '', status: 'running', result: '', isError: false };
    tools.set(callId, {
      ...previous,
      status: payload.is_error ? 'error' : 'done',
      result: resultText(payload.result),
      isError: Boolean(payload.is_error),
      latencyMs: payload.latency_ms,
    });
  }
}

/* ---------- 轨迹 ---------- */
export type TraceEvent = {
  sequence?: number;
  timestamp_ms?: number;
  trace_id?: string;
  session_id?: string;
  span_id?: string;
  kind: string;
  payload?: any;
};

export type Call = {
  id: string;
  sequence?: number;
  startedAt?: number;
  turn?: number;
  attempt?: number;
  modelId: string;
  request: Message[];
  toolNames: string[];
  output: Message | null;
  error: string | null;
  usage: { input_tokens?: number; output_tokens?: number } | null;
  finishReason: string;
  latencyMs: number;
  costUsd: number;
  // 已收到 model.response / model.error。
  responded: boolean;
  // 运行时 log_content=false：请求/响应正文被替换为占位文本。
  omitted: boolean;
};

const isOmitted = (value: unknown) => typeof value === 'string' && value.includes('content omitted');

export function projectTrajectory(events: TraceEvent[]): Call[] {
  const calls: Call[] = [];
  let current: Call | null = null;
  for (const event of events) {
    const payload = event.payload || {};
    if (event.kind === 'model.start') {
      current = {
        id: String(event.span_id || event.sequence),
        sequence: event.sequence,
        startedAt: event.timestamp_ms,
        turn: payload.turn, attempt: payload.attempt,
        modelId: payload.model_id || '',
        request: Array.isArray(payload.request?.messages) ? payload.request.messages : [],
        toolNames: Array.isArray(payload.request?.tools)
          ? payload.request.tools.map((t: any) => t?.function?.name).filter(Boolean) : [],
        output: null, error: null, usage: null, finishReason: '', latencyMs: 0, costUsd: 0,
        responded: false, omitted: isOmitted(payload.request),
      };
      calls.push(current);
    } else if (!current) {
      continue;
    } else if (event.kind === 'model.response') {
      current.responded = true;
      if (isOmitted(payload.message)) current.omitted = true;
      current.output = payload.message && typeof payload.message === 'object' ? payload.message : null;
      current.finishReason = payload.finish_reason || '';
      current.usage = payload.usage || null;
      current.latencyMs = payload.latency_ms || 0;
      current.costUsd = payload.cost_usd || 0;
      if (!current.modelId) current.modelId = payload.model_id || '';
    } else if (event.kind === 'model.error') {
      current.responded = true;
      current.error = typeof payload.error === 'string' ? payload.error : JSON.stringify(payload.error);
      current.latencyMs = payload.latency_ms || 0;
      current.modelId = payload.model_id || current.modelId;
    }
  }
  return calls;
}

// 消息 delta：第一条展示全量，其后只展示新增尾部并过滤 assistant
//（assistant 由上一卡的输出呈现）；消息数变少（压缩）则整段展示。
export function conversationDelta(previous: Message[] | null, current: Message[]) {
  if (!previous || current.length < previous.length) return current;
  if (current.length === previous.length) return [];
  return current.slice(previous.length).filter((message) => message.role !== 'assistant');
}
