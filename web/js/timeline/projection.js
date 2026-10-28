// 时间线投影：会话消息数组 + 实时工具状态 → 行模型（OpenCode timeline/projection 的移植）。
// 纯函数、无 DOM 依赖；渲染层按 row.key 复用。

// 工具卡状态：
//   streaming（流式参数中） / running（执行中） / completed / error
// rpi 会话消息没有工具态，完成态由后续 role=tool 消息配对得出。

function parseArgs(raw) {
  if (!raw) return {};
  if (typeof raw === 'object') return raw;
  try { return JSON.parse(raw); } catch (_) { return { _raw: String(raw) }; }
}

function resultIsError(content) {
  if (typeof content !== 'string') return false;
  return /"(?:isError|is_error)"\s*:\s*true/i.test(content) || /^\s*\{\s*"error"/.test(content);
}

export function resultText(content) {
  if (content === undefined || content === null) return '';
  if (typeof content === 'string') return content;
  try { return JSON.stringify(content, null, 2); } catch (_) { return String(content); }
}

// 便捷：从会话消息数组投影行。
// live 状态（可选）：{ reasoning, text, tools: Map(callId -> {name, args, status, result, isError, latencyMs}), running }
export function projectTimeline(messages, live = null) {
  const rows = [];
  const openTools = new Map(); // tool_call_id -> row
  let turn = 0;

  messages.forEach((message, index) => {
    const role = String(message.role || '').toLowerCase();
    if (role === 'system') return;

    if (role === 'user') {
      turn += 1;
      rows.push({ type: 'turn', key: `turn:${index}`, turn });
      rows.push({ type: 'user', key: `u:${index}`, text: typeof message.content === 'string' ? message.content : JSON.stringify(message.content, null, 2), message });
      return;
    }

    if (role === 'tool') {
      const row = openTools.get(message.tool_call_id);
      if (row) {
        row.result = resultText(message.content);
        row.status = resultIsError(message.content) ? 'error' : 'completed';
      } else {
        // 没有配对 call 的孤立工具输出（历史截断等）：保守渲染为折叠卡。
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
        : Array.isArray(message.content) ? message.content.map((p) => p?.text || '').join('\n') : '';
      if (text.trim()) {
        rows.push({
          type: 'text', key: `a:${index}:text`, text,
          model: message.model_id || null, streaming: false,
        });
      }
      const calls = Array.isArray(message.tool_calls) ? message.tool_calls : [];
      for (const call of calls) {
        const id = String(call?.id || `call:${index}:${rows.length}`);
        const row = {
          type: 'tool', key: `t:${id}`, callId: id,
          name: call?.function?.name || 'tool',
          args: parseArgs(call?.function?.arguments),
          input: typeof call?.function?.arguments === 'string' ? call.function.arguments : JSON.stringify(call?.function?.arguments ?? {}, null, 2),
          status: 'running', result: '', live: false,
        };
        openTools.set(id, row);
        rows.push(row);
      }
      return;
    }

    // 未知角色（error 等）作为系统注记。
    rows.push({ type: 'note', key: `n:${index}`, tone: role === 'error' ? 'error' : 'muted', text: typeof message.content === 'string' ? message.content : JSON.stringify(message.content) });
  });

  // 没有配对到 tool 消息的调用保持 running 态（会话被中断的情况）。
  // live 快照可以补充最终状态。
  if (live) {
    for (const row of rows) {
      if (row.type !== 'tool' || row.status !== 'running') continue;
      const snapshot = live.tools?.get(row.callId);
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

// 实时工具事件 Map 的构建（main.js 在 SSE trace 事件中维护）。
export function createLiveToolState() {
  const tools = new Map(); // callId -> {name, args, input, status, result, isError, latencyMs}
  return {
    tools,
    apply(event) {
      const payload = event.payload || {};
      const callId = String(payload.call_id || '');
      if (event.kind === 'tool.start' && callId) {
        tools.set(callId, {
          name: payload.name || 'tool',
          args: payload.arguments && typeof payload.arguments === 'object' ? payload.arguments : {},
          input: typeof payload.arguments === 'string' ? payload.arguments : JSON.stringify(payload.arguments ?? {}, null, 2),
          status: 'running', result: '', isError: false,
        });
      } else if (event.kind === 'tool.result' && callId) {
        const previous = tools.get(callId) || { name: payload.name || 'tool', args: {}, input: '' };
        previous.status = payload.is_error ? 'error' : 'done';
        previous.result = resultText(payload.result);
        previous.isError = Boolean(payload.is_error);
        previous.latencyMs = payload.latency_ms;
        tools.set(callId, previous);
      }
      return tools;
    },
  };
}

// 轨迹事件 → CallCard 模型（ZCode modelTrajectory + 消息 delta 去重的移植）。
export function projectTrajectory(events) {
  const calls = [];
  let current = null;
  for (const event of events) {
    const payload = event.payload || {};
    if (event.kind === 'model.start') {
      current = {
        id: String(event.span_id || event.sequence),
        sequence: event.sequence,
        startedAt: event.timestamp_ms,
        turn: payload.turn, attempt: payload.attempt,
        modelId: payload.model_id || '',
        reservedUsd: payload.reserved_usd,
        request: Array.isArray(payload.request?.messages) ? payload.request.messages : [],
        toolNames: Array.isArray(payload.request?.tools)
          ? payload.request.tools.map((t) => t?.function?.name).filter(Boolean) : [],
        output: null, error: null, usage: null, finishReason: '', latencyMs: 0, costUsd: 0,
      };
      calls.push(current);
    } else if (!current) {
      continue;
    } else if (event.kind === 'model.response') {
      current.output = payload.message && typeof payload.message === 'object' ? payload.message : null;
      current.finishReason = payload.finish_reason || '';
      current.usage = payload.usage || null;
      current.latencyMs = payload.latency_ms || 0;
      current.costUsd = payload.cost_usd || 0;
      if (!current.modelId) current.modelId = payload.model_id || '';
    } else if (event.kind === 'model.error') {
      current.error = typeof payload.error === 'string' ? payload.error : JSON.stringify(payload.error);
      current.latencyMs = payload.latency_ms || 0;
      current.modelId = payload.model_id || current.modelId;
    }
  }
  return calls;
}

// ZCode 消息 delta：第一条展示全量，其后只展示新增尾部并过滤 assistant
//（assistant 由上一卡的 Output 呈现）；消息数变少（压缩）则整段展示。
export function conversationDelta(previousMessages, currentMessages, { filterAssistant = true } = {}) {
  if (!previousMessages || currentMessages.length < previousMessages.length) return currentMessages;
  if (currentMessages.length === previousMessages.length) return [];
  return currentMessages.slice(previousMessages.length).filter((message) =>
    filterAssistant ? message.role !== 'assistant' : true);
}
