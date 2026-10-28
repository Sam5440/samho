(() => {
  'use strict';

  const byId = (id) => document.getElementById(id);
  const all = (selector) => Array.from(document.querySelectorAll(selector));
  const welcome = byId('welcome');
  const workspacePrefix = location.pathname.match(/^\/w\/[^/]+(?=\/|$)/)?.[0] || '';
  const apiPath = (path) => workspacePrefix + path;
  const state = {
    simulation: false, models: [], sessions: [], objects: [], skills: [], mcp: null,
    selectedSkills: new Set(), transcripts: new Map(), transcriptVersions: new Map(), currentSessionId: null,
    currentView: 'draft:0', draftCounter: 0, sessionRequest: 0,
    events: new Map(), traceId: '', traceStart: 0, task: null, pollTimer: null, stream: null,
    renderFrame: null, progressFrame: null, liveStream: null, apiOnline: false,
    ui: { theme: 'dark', submit_key: 'Ctrl+Enter', new_session_key: 'Alt+N', cancel_key: 'Ctrl+K' },
    tracePages: new Map(), pageLoading: false, tiers: [],
  };

  function node(tag, className, value) {
    const el = document.createElement(tag);
    if (className) el.className = className;
    if (value !== undefined && value !== null) el.textContent = String(value);
    return el;
  }

  function setText(id, value) {
    byId(id).textContent = value === undefined || value === null ? '' : String(value);
  }

  const tierNames = { nano: '轻量', fast: '快速', balanced: '均衡', reasoning: '推理', frontier: '前沿', specialist: '专家' };
  const kindNames = { agent: '智能体', model: '模型', tool: '工具', skill: '技能', mcp_server: 'MCP 服务', session: '会话', task: '任务', plugin: '插件', custom: '自定义', object: '对象' };
  const statusNames = { ready: '就绪', running: '执行中', completed: '已完成', done: '已完成', failed: '失败', cancelled: '已取消', interrupted: '已中断', connected: '已连接', degraded: '能力降级', disabled: '已禁用', closed: '已关闭', unknown: '未知' };
  const eventNames = { 'task.start': '任务开始', 'task.done': '任务完成', 'task.failed': '任务失败', 'task.cancelled': '任务取消', 'model.route': '模型路由', 'model.start': '模型调用开始', 'model.response': '模型响应', 'model.error': '模型调用失败', 'tool.start': '工具调用开始', 'tool.result': '工具结果', 'skill.load': '加载技能' };
  const tierName = (value) => tierNames[value] || value;
  const statusName = (value) => statusNames[value] || '未知状态';

  function array(value, key) {
    if (Array.isArray(value)) return value;
    if (!value || typeof value !== 'object') return [];
    if (key && Array.isArray(value[key])) return value[key];
    return Array.isArray(value.items) ? value.items : Array.isArray(value.data) ? value.data : [];
  }

  function pretty(value) {
    if (value === undefined || value === null) return '';
    if (typeof value === 'string') return value;
    try { return JSON.stringify(value, null, 2); } catch (_) { return String(value); }
  }

  function contentText(value) {
    if (typeof value === 'string') return value;
    if (Array.isArray(value)) return value.map((part) => part.text || pretty(part)).join('\n');
    return pretty(value);
  }

  function errorMessage(error) {
    return error && error.message ? error.message : String(error);
  }

  async function api(path, options = {}) {
    const headers = { Accept: 'application/json', ...(options.headers || {}) };
    const response = await fetch(apiPath(path), { ...options, headers });
    const raw = await response.text();
    let body = null;
    if (raw) { try { body = JSON.parse(raw); } catch (_) { body = raw; } }
    if (!response.ok) {
      const detail = body && typeof body === 'object' ? body.error || body.message : body;
      const error = new Error(detail ? contentText(detail) : String(response.status) + ' ' + response.statusText);
      error.status = response.status;
      throw error;
    }
    state.apiOnline = true;
    setConnection(true);
    return body;
  }

  function setConnection(online) {
    const dot = byId('connection-dot');
    dot.classList.toggle('online', online);
    dot.classList.toggle('offline', !online);
    setText('connection-label', online ? '运行时已连接' : '运行时不可达');
  }

  function showError(message) {
    setText('global-error', message);
    byId('global-error').hidden = !message;
  }

  function sessionId(item) {
    return typeof item === 'string' ? item : String(item && (item.id || item.session_id || item.sessionId) || '');
  }

  function sessionTitle(item) {
    if (typeof item === 'string') return item;
    return String(item && (item.title || item.name || item.label || item.id || item.session_id) || '未命名会话');
  }

  function dateLabel(value) {
    if (!value) return '';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
  }

  function sessionStatus(item) {
    const s = typeof item === 'object' && item ? String(item.status || 'ready') : 'ready';
    if (s === 'running' || s === 'queued') return { label: '运行中', cls: 'st-running', icon: '◐' };
    if (s === 'failed') return { label: '失败', cls: 'st-failed', icon: '✕' };
    if (s === 'interrupted') return { label: '已中断', cls: 'st-interrupted', icon: '⏸' };
    if (s === 'done' || s === 'completed') return { label: '已完成', cls: 'st-done', icon: '○' };
    return { label: '就绪', cls: 'st-ready', icon: '○' };
  }

  function renderSessions() {
    const list = byId('session-list');
    list.replaceChildren();
    setText('session-count', state.sessions.length);
    if (!state.sessions.length) {
      list.append(node('p', 'empty-note', '还没有会话。运行第一个任务。'));
      return;
    }
    state.sessions.forEach((session) => {
      const id = sessionId(session);
      const selected = id === state.currentSessionId;
      const st = sessionStatus(session);
      const button = node('button', 'session-item' + (selected ? ' selected' : ''));
      button.type = 'button';
      button.setAttribute('aria-current', selected ? 'page' : 'false');
      button.title = sessionTitle(session);
      button.append(node('span', 'session-icon', selected ? '◉' : st.icon));
      const copy = node('span', 'session-copy');
      copy.append(node('span', 'session-title', sessionTitle(session)));
      const stamp = typeof session === 'object' && (session.updated_at || session.updated_at_ms || session.created_at || session.created_at_ms);
      const subtitle = node('span', 'session-subtitle');
      subtitle.append(node('span', 'session-status ' + st.cls, st.label + ' · '));
      subtitle.append(node('span', 'session-stamp', dateLabel(stamp) || id.slice(0, 12)));
      copy.append(subtitle);
      button.append(copy);
      button.addEventListener('click', () => loadSession(id));
      list.append(button);
    });
  }

  function messagesFor(view = state.currentView) {
    if (!state.transcripts.has(view)) state.transcripts.set(view, []);
    return state.transcripts.get(view);
  }

  function renderMessages() {
    const container = byId('messages');
    const messages = messagesFor();
    const nearBottom = container.scrollTop + container.clientHeight >= container.scrollHeight - 80;
    container.replaceChildren();
    if (!messages.length) { container.append(welcome); return; }
    messages.forEach((message) => {
      const rawRole = String(message.role || 'assistant').toLowerCase();
      const role = ['user', 'assistant', 'tool', 'system', 'error'].includes(rawRole) ? rawRole : 'assistant';
      const item = node('article', 'message ' + role);
      const head = node('div', 'message-head');
      const names = { user: '你', assistant: '智能体', tool: '工具输出', system: '运行时', error: '执行错误' };
      head.append(node('span', 'message-avatar', role === 'user' ? '你' : role === 'tool' ? '∷' : 'rπ'), node('span', '', names[role]));
      const stamp = message.timestamp || message.timestamp_ms || message.created_at;
      if (stamp) head.append(node('time', '', dateLabel(stamp)));
      const content = contentText(message.content !== undefined ? message.content : message.text !== undefined ? message.text : message);
      item.append(head, node('div', 'message-content', content));
      const metadata = [];
      if (message.model_id) metadata.push('模型 ' + message.model_id);
      if (message.simulation) metadata.push('模拟模式');
      if (message.trace_id) metadata.push('轨迹 ' + message.trace_id);
      if (metadata.length) item.append(node('div', 'message-meta', metadata.join('  ·  ')));
      container.append(item);
    });
    renderLiveMessage(container);
    if (nearBottom) requestAnimationFrame(() => { container.scrollTop = container.scrollHeight; });
  }

  function renderLiveMessage(container) {
    const task = state.task;
    if (!task || task.view !== state.currentView || !task.progress) return;
    const progress = task.progress;
    const item = node('article', 'message assistant live-message');
    const head = node('div', 'message-head');
    head.append(node('span', 'message-avatar', 'rπ'), node('span', '', '智能体 · 实时输出'), node('span', 'live-badge', '第 ' + (progress.turn || 1) + ' 轮 / 尝试 ' + (progress.attempt || 1)));
    item.append(head, node('div', 'message-content', progress.text || '等待模型输出…'));
    if (progress.reasoning) {
      const details = node('details', 'live-reasoning');
      details.append(node('summary', '', '查看推理过程'), node('pre', '', progress.reasoning));
      item.append(details);
    }
    const tools = array(progress.tool_calls);
    if (tools.length) {
      const details = node('details', 'live-reasoning');
      details.append(node('summary', '', '工具调用 · ' + tools.length), node('pre', '', pretty(tools)));
      item.append(details);
    }
    if (progress.model_id) item.append(node('div', 'message-meta', '模型 ' + progress.model_id + (progress.trace_id ? ' · 轨迹 ' + progress.trace_id : '')));
    container.append(item);
  }

  function applyProgress(progress) {
    const task = state.task;
    if (!task || !progress || String(progress.task_id) !== task.id) return;
    if (task.progress && Number(progress.revision) <= Number(task.progress.revision)) return;
    // The server sends cumulative snapshots. Replace rather than append so
    // retries, duplicated SSE delivery and polling recovery cannot duplicate text.
    task.progress = progress;
    if (task.view === state.currentView && progress.trace_id) state.traceId = String(progress.trace_id);
    if (!state.progressFrame) state.progressFrame = requestAnimationFrame(() => {
      state.progressFrame = null; renderMessages(); updateTaskUI(); renderTrace();
    });
  }

  function renderTierOptions() {
    const select = byId('tier-select');
    const previous = select.value;
    select.replaceChildren(new Option('自动路由', ''));
    const tiers = state.tiers.length ? state.tiers : Object.keys(tierNames).map((id) => ({ id, name: tierNames[id] }));
    tiers.forEach((tier) => {
      const id = String(tier.id || tier.name || '');
      if (!id) return;
      const label = tier.label || tier.name || tierName(id);
      select.append(new Option(label + (tier.min_complexity === undefined ? '' : ' · 复杂度 ' + tier.min_complexity + '–' + tier.max_complexity), id));
    });
    if (Array.from(select.options).some((option) => option.value === previous)) select.value = previous;
  }

  function addMessage(view, role, content, metadata = {}) {
    messagesFor(view).push({ role, content, timestamp_ms: Date.now(), ...metadata });
    state.transcriptVersions.set(view, (state.transcriptVersions.get(view) || 0) + 1);
    if (view === state.currentView) {
      renderMessages();
      requestAnimationFrame(() => { const el = byId('messages'); el.scrollTop = el.scrollHeight; });
    }
  }

  function isSimulationModel(model) {
    return state.simulation || String(model.provider || '').toLowerCase() === 'demo';
  }

  function renderModelOptions() {
    const select = byId('model-select');
    const previous = select.value;
    const tier = byId('tier-select').value;
    select.replaceChildren(new Option('由路由器选择', ''));
    state.models.filter((model) => !tier || String(model.tier).toLowerCase() === tier).forEach((model) => {
      const label = [model.id, tierName(model.tier), isSimulationModel(model) ? '模拟模式' : '真实供应商'].filter(Boolean).join(' · ');
      select.append(new Option(label, String(model.id)));
    });
    if (Array.from(select.options).some((option) => option.value === previous)) select.value = previous;
    updateSelectionSummary();
  }

  function updateSelectionSummary() {
    const tier = byId('tier-select').value;
    const model = byId('model-select').value;
    setText('selection-summary', model ? model : tier ? tierName(tier) + ' / 自动选择' : byId('automatic').checked ? '按任务自动路由' : '按复杂度路由');
    byId('selection-summary').title = model ? '指定模型：' + model : '按任务复杂度与模型能力路由';
    setText('complexity-value', byId('complexity').value);
    setText('selected-skill-count', state.selectedSkills.size);
  }

  function renderSkills() {
    const list = byId('skill-list');
    list.replaceChildren();
    setText('skill-count', state.skills.length + ' 个技能');
    if (!state.skills.length) { list.append(node('div', 'registry-empty', '尚未发现技能。运行时会从配置目录加载 SKILL.md。')); return; }
    state.skills.forEach((skill) => {
      const name = String(skill.name || skill.id || '').replace(/^skill:/, '');
      if (!name) return;
      const item = node('div', 'registry-item');
      const label = node('label', 'skill-label');
      const checkbox = document.createElement('input');
      checkbox.type = 'checkbox';
      checkbox.checked = state.selectedSkills.has(name);
      checkbox.addEventListener('change', () => {
        if (checkbox.checked) state.selectedSkills.add(name); else state.selectedSkills.delete(name);
        updateSelectionSummary();
      });
      label.append(checkbox, node('span', '', name));
      item.append(label);
      if (skill.description) item.append(node('p', 'registry-description', skill.description));
      if (skill.path) item.append(node('div', 'skill-path', skill.path));
      if (skill.instructions) {
        const details = node('details', 'skill-instructions');
        details.append(node('summary', '', '查看指令'), node('pre', 'registry-detail', skill.instructions));
        item.append(details);
      }
      list.append(item);
    });
  }

  function objectCard(object) {
    const details = node('details', 'registry-item');
    const summary = document.createElement('summary');
    const kind = String(object.kind || object.type || 'object');
    summary.append(node('span', 'registry-kind', kindNames[kind] || '自定义对象'), node('span', 'registry-name', object.id || object.name || '未命名对象'));
    details.append(summary);
    if (object.description) details.append(node('p', 'registry-description', object.description));
    if (object.version !== undefined) details.append(node('div', 'skill-path', '版本 / ' + object.version));
    const record = { labels: object.labels, dependencies: object.dependencies, capabilities: object.capabilities, spec: object.spec };
    details.append(node('pre', 'registry-detail', pretty(record)));
    return details;
  }

  function renderObjects() {
    const list = byId('object-list');
    list.replaceChildren();
    setText('object-count', state.objects.length + ' 个对象');
    if (!state.objects.length) list.append(node('div', 'registry-empty', '对象注册表为空。'));
    state.objects.forEach((object) => list.append(objectCard(object)));
    const mcp = state.objects.filter((object) => /mcp/i.test(String(object.kind || '') + ' ' + String(object.id || '')) || object.spec && object.spec.protocol === 'mcp');
    const mcpList = byId('mcp-list');
    mcpList.replaceChildren();
    const servers = state.mcp && Array.isArray(state.mcp.servers) ? state.mcp.servers : [];
    if (!mcp.length && !servers.length) mcpList.append(node('div', 'registry-empty', '未发现 MCP 服务或工具对象。请在运行时配置 MCP 服务。'));
    servers.forEach((server) => {
      const card = node('div', 'registry-item');
      const status = String(server.state || 'unknown');
      card.append(node('span', 'registry-kind', 'MCP 服务 · ' + statusName(status)), node('span', 'registry-name', server.name || '未命名服务'));
      if (server.error) card.append(node('p', 'registry-description', contentText(server.error)));
      const facts = { transport: server.transport, tool_count: server.tool_count, server_info: server.server_info };
      card.append(node('pre', 'registry-detail', pretty(facts)));
      mcpList.append(card);
    });
    mcp.forEach((object) => mcpList.append(objectCard(object)));
  }

  function visibleEvents() {
    return Array.from(state.events.values()).filter((event) => {
      if (state.traceId) return event.trace_id === state.traceId;
      if (state.currentSessionId && event.session_id !== state.currentSessionId) return false;
      return !state.traceStart || Number(event.timestamp_ms || 0) >= state.traceStart;
    }).sort((a, b) => Number(a.sequence || 0) - Number(b.sequence || 0));
  }

  function updateTraceOptions() {
    const select = byId('trace-select');
    const ids = Array.from(new Set(Array.from(state.events.values()).map((event) => event.trace_id).filter(Boolean)));
    if (state.traceId && !ids.includes(state.traceId)) ids.push(state.traceId);
    const viewLabel = state.currentSessionId ? '当前会话 · ' : '工作台 · ';
    select.replaceChildren(new Option(viewLabel + (state.traceStart ? '新轨迹' : '全部轨迹'), ''));
    ids.slice().reverse().forEach((id) => select.append(new Option(id, id)));
    select.value = state.traceId;
  }

  function renderTrace() {
    state.renderFrame = null;
    const list = byId('trace-list');
    const events = visibleEvents();
    const opened = new Set(Array.from(list.querySelectorAll('details[open]')).map((item) => item.dataset.eventId));
    list.replaceChildren();
    updateTraceOptions();
    setText('trace-count', events.length + ' 个事件');
    byId('export-trace').disabled = !state.traceId;
    const page = state.tracePages.get(state.traceId || '');
    byId('load-more-events').disabled = state.pageLoading;
    byId('load-more-events').textContent = page && !page.hasMore ? '刷新轨迹' : '加载后续事件';
    if (!events.length) {
      const placeholder = node('div', 'trace-placeholder');
      placeholder.append(node('div', 'trace-placeholder-line'), node('span', '', '等待执行信号'), node('small', '', '从任务到结果，完整记录。'));
      list.append(placeholder);
      setText('trace-summary', state.traceId ? '轨迹 / ' + state.traceId : '运行任务后，在此查看模型路由与调用链。');
      return;
    }
    const origin = Number(events[0].timestamp_ms || 0);
    const parents = new Map(events.filter((event) => event.span_id).map((event) => [event.span_id, event.parent_span_id]));
    events.slice(-200).forEach((event) => {
      const item = node('details', 'trace-item' + (/error|fail/i.test(String(event.kind)) ? ' error' : ''));
      const id = eventIdentity(event);
      item.dataset.eventId = id;
      let parent = event.parent_span_id, depth = 0;
      const visited = new Set([event.span_id]);
      while (parent && !visited.has(parent) && depth < 8) { visited.add(parent); depth += 1; parent = parents.get(parent); }
      item.style.setProperty('--span-depth', String(depth));
      item.open = opened.has(id);
      const summary = document.createElement('summary');
      const elapsed = Number(event.timestamp_ms || origin) - origin;
      summary.append(node('span', 'trace-kind', eventNames[event.kind] || event.kind || '轨迹事件'), node('span', 'trace-time', '+' + elapsed.toLocaleString() + ' 毫秒'));
      item.append(summary);
      if (event.object_id) item.append(node('span', 'trace-object', '对象 / ' + event.object_id));
      item.append(node('pre', '', pretty({ sequence: event.sequence, timestamp_ms: event.timestamp_ms, trace_id: event.trace_id, session_id: event.session_id, span_id: event.span_id, parent_span_id: event.parent_span_id, object_id: event.object_id, payload: event.payload })));
      list.append(item);
    });
    const suffix = events.length > 200 ? ' · 显示最近 200 条' : '';
    setText('trace-summary', (state.traceId ? '轨迹 / ' + state.traceId : state.currentSessionId ? '当前会话调用链' : '工作台调用链') + '\n' + events.length + ' 个事件' + suffix);
  }

  function eventIdentity(event) {
    return String(event.trace_id || '') + '/' + String(event.sequence === undefined ? '' : event.sequence) + '/' + String(event.span_id || '') + '/' + String(event.kind || '');
  }

  function appendTrace(event) {
    if (!event || typeof event !== 'object' || !event.kind) return;
    state.events.set(eventIdentity(event), event);
    if (state.events.size > 5000) state.events.delete(state.events.keys().next().value);
    if (!state.renderFrame) state.renderFrame = requestAnimationFrame(renderTrace);
  }

  async function recoverTrace(traceId = state.traceId) {
    if (state.pageLoading) return;
    state.pageLoading = true;
    byId('load-more-events').disabled = true;
    try {
      const page = state.tracePages.get(traceId || '') || { cursor: 0, hasMore: true };
      const history = await api('/api/events/page?after=' + page.cursor + '&limit=100' + (traceId ? '&trace_id=' + encodeURIComponent(traceId) : ''));
      array(history, 'events').forEach(appendTrace);
      state.tracePages.set(traceId || '', { cursor: Number(history.next_cursor || page.cursor), hasMore: Boolean(history.has_more) });
      setText('page-status', history.has_more ? '还有事件，可继续加载' : '已加载至当前末尾');
      if (Number(history.first_available_sequence) > page.cursor + 1 && page.cursor > 0) setText('page-status', '较早记录已归档；从可用位置继续');
    } catch (error) { setText('page-status', '读取轨迹失败：' + errorMessage(error)); }
    finally { state.pageLoading = false; renderTrace(); }
  }

  function attachStream() {
    if (!window.EventSource) { setText('stream-status', '事件流 / 浏览器不支持'); return; }
    if (state.stream) state.stream.close();
    const source = new EventSource(apiPath('/api/events/stream'));
    state.stream = source;
    source.onopen = () => setText('stream-status', '事件流 / 已连接');
    source.onerror = () => setText('stream-status', '事件流 / 重连中');
    source.addEventListener('trace', (event) => {
      try { appendTrace(JSON.parse(event.data)); } catch (_) { setText('stream-status', '事件流 / 无效事件'); }
    });
    source.addEventListener('gap', () => { recoverTrace(); });
    if (state.liveStream) state.liveStream.close();
    const live = new EventSource(apiPath('/api/live/stream'));
    state.liveStream = live;
    live.addEventListener('progress', (event) => {
      try { applyProgress(JSON.parse(event.data)); } catch (_) { setText('stream-status', '实时输出 / 无效快照'); }
    });
    live.addEventListener('gap', () => recoverProgress());
    live.onopen = () => recoverProgress();
  }

  async function recoverProgress() {
    const task = state.task;
    if (!task || !task.id) return;
    try { const snapshot = await api('/api/tasks/' + encodeURIComponent(task.id)); if (state.task === task) applyProgress(snapshot.progress); } catch (_) { /* regular polling will retry */ }
  }

  function updateTaskUI(status) {
    const running = Boolean(state.task);
    const label = byId('task-state');
    const phase = running ? 'running' : status || 'ready';
    setText('task-state', statusName(phase));
    label.classList.toggle('running', running);
    label.classList.toggle('failed', phase === 'failed');
    byId('execution-status').hidden = !running;
    byId('control-mode').hidden = !running;
    byId('submit-task').disabled = !byId('prompt').value.trim() || !state.apiOnline || (running && (!state.task.id || state.task.cancelRequested || state.task.sendingControl));
    byId('submit-task').querySelector('span').textContent = running ? (byId('send-mode').value === 'steer' ? '调整方向' : '追加任务') : '运行任务';
    byId('fork-session').disabled = !state.currentSessionId;
    byId('cancel-task').disabled = !running || !state.task.id || state.task.cancelRequested;
    if (running) {
      const away = state.task.view !== state.currentView ? ' · 另一个会话' : '';
      const caption = !state.task.id ? '正在提交任务…' : state.task.cancelRequested ? '等待运行时取消任务…' : '智能体正在执行任务…';
      setText('execution-label', caption + away);
    }
  }

  async function refreshSessions(reportError = false) {
    try {
      state.sessions = array(await api('/api/sessions'), 'sessions').filter((session) => sessionId(session));
      renderSessions();
    } catch (error) { if (reportError) showError('会话刷新失败：' + errorMessage(error)); }
  }

  async function finishTask(response, task) {
    if (state.task !== task) return;
    const result = response.result || null;
    const status = response.status || 'failed';
    const originalView = task.view;
    task.progress = null;
    const resultSession = result && result.session_id;
    if (resultSession) {
      const newView = 'session:' + resultSession;
      state.transcripts.set(newView, messagesFor(originalView));
      state.transcriptVersions.set(newView, state.transcriptVersions.get(originalView) || 0);
      if (newView !== originalView) { state.transcripts.delete(originalView); state.transcriptVersions.delete(originalView); }
      task.view = newView;
      if (state.currentView === originalView) {
        state.currentView = newView;
        state.currentSessionId = String(resultSession);
        if (!state.sessions.some((session) => sessionId(session) === String(resultSession))) state.sessions.unshift({ id: String(resultSession), title: task.prompt.slice(0, 48) });
        setText('conversation-title', sessionTitle(state.sessions.find((session) => sessionId(session) === String(resultSession))));
      }
    }
    if (status === 'completed') {
      const selectedModel = state.models.find((model) => result && model.id === result.model_id) || {};
      addMessage(task.view, 'assistant', result && result.output !== undefined ? contentText(result.output) : '任务已完成，运行时未返回文本输出。', { model_id: result && result.model_id, trace_id: result && result.trace_id, simulation: isSimulationModel(selectedModel) });
    } else if (status === 'cancelled') {
      addMessage(task.view, 'system', '任务已取消。');
    } else {
      addMessage(task.view, 'error', response.error || result && result.error || '任务执行失败。');
    }
    state.task = null;
    clearTimeout(state.pollTimer);
    state.pollTimer = null;
    updateTaskUI(status);
    renderMessages();
    renderSessions();
    if (result && result.trace_id) {
      if (state.currentView === task.view) { state.traceId = String(result.trace_id); renderTrace(); }
      recoverTrace(String(result.trace_id));
    }
    const meta = [];
    if (result && result.model_id) meta.push(result.model_id);
    if (result && result.turns !== undefined) meta.push(String(result.turns) + ' 轮调用');
    if (result && result.usage) {
      const usage = result.usage;
      const total = usage.total_tokens !== undefined ? usage.total_tokens : Number(usage.input_tokens || 0) + Number(usage.output_tokens || 0);
      if (total) meta.push(String(total) + ' 个词元');
    }
    setText('last-run-meta', meta.length ? meta.join(' · ') : statusName(status));
    await refreshSessions();
  }

  async function pollTask(task) {
    if (state.task !== task || !task.id) return;
    try {
      const response = await api('/api/tasks/' + encodeURIComponent(task.id));
      if (state.task !== task) return;
      applyProgress(response.progress);
      if (task.failures) showError('');
      task.failures = 0;
      if (response && ['completed', 'failed', 'cancelled'].includes(response.status)) {
        await finishTask(response, task);
        return;
      }
      updateTaskUI();
      state.pollTimer = setTimeout(() => pollTask(task), 800);
    } catch (error) {
      if (state.task !== task) return;
      if (error.status === 404) {
        await finishTask({ status: 'failed', error: '任务索引已不可用，服务可能已重启。请刷新历史会话并检查执行轨迹。' }, task);
        return;
      }
      task.failures += 1;
      showError('暂时无法读取任务状态，正在重试：' + errorMessage(error));
      if (task.failures >= 3) setConnection(false);
      state.pollTimer = setTimeout(() => pollTask(task), Math.min(5000, task.failures * 1000));
    }
  }

  async function submitTask(event) {
    event.preventDefault();
    const prompt = byId('prompt').value.trim();
    if (!prompt) return;
    if (state.task) { await submitControl(prompt); return; }
    showError('');
    const task = { id: null, view: state.currentView, prompt, failures: 0, cancelRequested: false };
    state.task = task;
    state.traceId = '';
    state.traceStart = Date.now();
    renderTrace();
    addMessage(task.view, 'user', prompt);
    setText('conversation-title', state.currentSessionId ? byId('conversation-title').textContent : prompt.slice(0, 52));
    byId('prompt').value = '';
    updateTaskUI();
    const body = {
      session_id: state.currentSessionId,
      prompt,
      profile: { automatic: byId('automatic').checked, complexity: Number(byId('complexity').value), tier: byId('tier-select').value || null, model: byId('model-select').value || null },
      skills: Array.from(state.selectedSkills),
    };
    try {
      const submission = await api('/api/run', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      if (!submission || !submission.task_id) throw new Error('运行时没有返回 task_id。');
      task.id = String(submission.task_id);
      updateTaskUI();
      recoverTrace();
      pollTask(task);
    } catch (error) {
      await finishTask({ status: 'failed', error: '任务提交失败：' + errorMessage(error) }, task);
      byId('prompt').value = prompt;
      updateTaskUI('failed');
    }
  }

  async function submitControl(prompt) {
    const task = state.task;
    if (!task || !task.id || task.sendingControl || task.cancelRequested) return;
    const kind = byId('send-mode').value === 'follow-up' ? 'follow-up' : 'steer';
    task.sendingControl = true; updateTaskUI();
    try {
      const queued = await api('/api/tasks/' + encodeURIComponent(task.id) + '/' + kind, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ prompt }) });
      addMessage(task.view, 'system', (kind === 'steer' ? '已排队调整方向：' : '已排队追加任务：') + prompt + '\n队列序号 ' + queued.sequence);
      byId('prompt').value = ''; setText('queue-status', '已进入队列，序号 ' + queued.sequence);
    } catch (error) { showError('任务调整失败：' + errorMessage(error)); }
    finally { if (state.task === task) { task.sendingControl = false; updateTaskUI(); } }
  }

  async function forkSession() {
    if (!state.currentSessionId) return;
    const cutoffText = byId('fork-cutoff').value.trim();
    const body = cutoffText ? { cutoff: Number(cutoffText) } : {};
    if (cutoffText && (!Number.isSafeInteger(body.cutoff) || body.cutoff < 0)) { showError('保留消息数必须是非负整数。'); return; }
    byId('fork-session').disabled = true;
    try {
      const fork = await api('/api/sessions/' + encodeURIComponent(state.currentSessionId) + '/fork', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      await refreshSessions(); await loadSession(sessionId(fork));
    } catch (error) { showError('创建会话分支失败：' + errorMessage(error)); }
    finally { updateTaskUI(); }
  }

  async function cancelTask() {
    const task = state.task;
    if (!task || !task.id || task.cancelRequested) return;
    task.cancelRequested = true;
    updateTaskUI();
    try {
      await api('/api/tasks/' + encodeURIComponent(task.id) + '/cancel', { method: 'POST' });
    } catch (error) {
      if (state.task === task) { task.cancelRequested = false; updateTaskUI(); }
      showError('取消任务失败：' + errorMessage(error));
    }
  }

  async function loadSession(id) {
    const request = ++state.sessionRequest;
    showError('');
    const view = 'session:' + id;
    const version = state.transcriptVersions.get(view) || 0;
    state.currentSessionId = id;
    state.currentView = view;
    state.traceId = '';
    state.traceStart = 0;
    setText('conversation-title', sessionTitle(state.sessions.find((session) => sessionId(session) === id)));
    renderSessions(); renderMessages(); updateTaskUI();
    try {
      const detail = await api('/api/sessions/' + encodeURIComponent(id));
      if (request !== state.sessionRequest) return;
      const pending = state.task && state.task.view === view;
      if (!pending && version === (state.transcriptVersions.get(view) || 0)) state.transcripts.set(view, array(detail && (detail.messages || detail.history), 'messages'));
      if (detail && (detail.title || detail.name)) setText('conversation-title', detail.title || detail.name);
      const history = messagesFor(view);
      const latest = history.slice().reverse().find((message) => message.trace_id);
      state.traceId = String(detail && detail.trace_id || latest && latest.trace_id || '');
      renderMessages(); renderTrace();
      recoverTrace(state.traceId);
    } catch (error) {
      if (request === state.sessionRequest) showError('无法读取会话：' + errorMessage(error));
    }
  }

  function applyUiConfig(value) {
    if (!value || typeof value !== 'object') return;
    state.ui = { ...state.ui, ...value };
    byId('ui-theme').value = state.ui.theme || 'dark';
    byId('ui-submit-key').value = state.ui.submit_key || 'Ctrl+Enter';
    byId('ui-new-key').value = state.ui.new_session_key || 'Alt+N';
    byId('ui-cancel-key').value = state.ui.cancel_key || 'Ctrl+K';
    document.documentElement.dataset.theme = state.ui.theme === 'system' ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : state.ui.theme || 'dark';
    byId('new-session').querySelector('kbd').textContent = state.ui.new_session_key;
    document.querySelector('.send-shortcut').textContent = state.ui.submit_key;
  }

  function matchesShortcut(event, shortcut) {
    if (event.isComposing || event.repeat) return false;
    const parts = String(shortcut || '').toLowerCase().split('+').map((part) => part.trim());
    const key = parts.pop();
    const ctrl = parts.includes('ctrl') || parts.includes('control');
    const meta = parts.includes('meta') || parts.includes('cmd') || parts.includes('command');
    const shift = parts.includes('shift'), alt = parts.includes('alt');
    // Ctrl shortcuts also work with Command on macOS unless Meta is explicit.
    const command = ctrl && !meta ? event.ctrlKey || event.metaKey : event.ctrlKey === ctrl && event.metaKey === meta;
    return command && event.altKey === alt && event.shiftKey === shift && event.key.toLowerCase() === key;
  }

  async function loadWorkspaces() {
    try {
      const workspaces = array(await api('/api/workspaces'));
      const select = byId('workspace-select');
      if (workspaces.length > 1) {
        select.hidden = false; select.replaceChildren(...workspaces.map((workspace) => new Option(workspace.name || workspace.id, workspace.id)));
        const current = workspacePrefix.match(/^\/w\/([^/]+)/)?.[1]; if (current) select.value = current;
        select.addEventListener('change', () => { const chosen = workspaces.find((workspace) => String(workspace.id) === select.value); if (chosen && chosen.url) location.href = chosen.url; });
      }
    } catch (_) { /* single-workspace runtimes may not expose discovery */ }
  }

  async function saveUiConfig(event) {
    event.preventDefault();
    const value = { theme: byId('ui-theme').value, submit_key: byId('ui-submit-key').value.trim(), new_session_key: byId('ui-new-key').value.trim(), cancel_key: byId('ui-cancel-key').value.trim() };
    if (!value.submit_key || !value.new_session_key || !value.cancel_key) { setText('settings-status', '快捷键不能为空。'); return; }
    try { applyUiConfig(await api('/api/ui', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value) })); setText('settings-status', '已保存到当前工作区。'); } catch (error) { setText('settings-status', '保存失败：' + errorMessage(error)); }
  }

  function newSession() {
    ++state.sessionRequest;
    state.currentSessionId = null;
    state.currentView = 'draft:' + (++state.draftCounter);
    state.traceId = '';
    state.traceStart = Date.now();
    setText('conversation-title', '从一个任务开始');
    showError('');
    renderMessages(); renderSessions(); renderTrace(); updateTaskUI();
    byId('prompt').focus();
  }

  function showPanel(name, focus = false) {
    all('.inspector-tabs button').forEach((button) => {
      const active = button.dataset.panel === name;
      button.classList.toggle('active', active);
      button.setAttribute('aria-selected', String(active));
      button.tabIndex = active ? 0 : -1;
      if (active && focus) button.focus();
    });
    all('.inspector-panel').forEach((panel) => { panel.hidden = panel.id !== 'panel-' + name; });
  }

  async function bootstrap() {
    const names = ['状态', '模型', '会话', '对象', '技能', '界面'];
    const results = await Promise.allSettled(['/api/state', '/api/models', '/api/sessions', '/api/objects', '/api/skills', '/api/ui'].map((path) => api(path)));
    const value = (index) => results[index].status === 'fulfilled' ? results[index].value : null;
    const runtime = value(0);
    if (runtime) {
      state.simulation = Boolean(runtime.simulation);
      state.mcp = runtime.mcp || null;
      state.tiers = array(runtime.tiers);
      renderTierOptions();
      const workspace = runtime.workspace || runtime.workspace_name || '本地工作区';
      setText('workspace-name', typeof workspace === 'string' ? workspace : workspace.path || workspace.name || pretty(workspace));
      byId('workspace-name').title = byId('workspace-name').textContent;
      byId('simulation-banner').hidden = !state.simulation;
      setText('runtime-caption', state.simulation ? '模拟模型 · Rust 运行时' : 'Rust 运行时 · 可观测');
    } else {
      setText('workspace-name', '运行时状态不可用');
      setText('runtime-caption', '运行时状态不可用');
    }
    state.models = array(value(1), 'models').filter((model) => model && model.id);
    state.sessions = array(value(2), 'sessions').filter((session) => sessionId(session));
    state.objects = array(value(3), 'objects');
    state.skills = array(value(4), 'skills');
    applyUiConfig(value(5));
    renderModelOptions(); renderSessions(); renderObjects(); renderSkills(); updateTaskUI();
    const failed = results.map((result, index) => result.status === 'rejected' ? names[index] : null).filter(Boolean);
    if (failed.length) showError('无法读取：' + failed.join('、') + '。请确认本地服务已启动，然后刷新页面。');
    setConnection(state.apiOnline);
    attachStream();
    if (state.apiOnline) recoverTrace();
    loadWorkspaces();
  }

  byId('composer').addEventListener('submit', submitTask);
  byId('ui-settings').addEventListener('submit', saveUiConfig);
  byId('open-settings').addEventListener('click', () => { byId('ui-settings').hidden = !byId('ui-settings').hidden; });
  byId('close-settings').addEventListener('click', () => { byId('ui-settings').hidden = true; });
  byId('fork-session').addEventListener('click', forkSession);
  byId('send-mode').addEventListener('change', updateTaskUI);
  byId('load-more-events').addEventListener('click', () => recoverTrace(state.traceId));
  byId('export-trace').addEventListener('click', () => { if (state.traceId) window.open(apiPath('/api/traces/' + encodeURIComponent(state.traceId) + '/export'), '_blank', 'noopener'); });
  byId('cancel-task').addEventListener('click', cancelTask);
  byId('new-session').addEventListener('click', newSession);
  byId('refresh-sessions').addEventListener('click', () => refreshSessions(true));
  byId('tier-select').addEventListener('change', renderModelOptions);
  byId('model-select').addEventListener('change', updateSelectionSummary);
  byId('automatic').addEventListener('change', updateSelectionSummary);
  byId('complexity').addEventListener('input', () => { byId('automatic').checked = false; updateSelectionSummary(); });
  byId('open-skills').addEventListener('click', () => showPanel('skills', true));
  byId('trace-select').addEventListener('change', (event) => { state.traceId = event.target.value; renderTrace(); recoverTrace(); });
  byId('prompt').addEventListener('input', () => updateTaskUI());
  document.addEventListener('keydown', (event) => {
    if (matchesShortcut(event, state.ui.submit_key) && event.target === byId('prompt')) { event.preventDefault(); byId('composer').requestSubmit(); }
    else if (matchesShortcut(event, state.ui.new_session_key)) { event.preventDefault(); newSession(); }
    else if (matchesShortcut(event, state.ui.cancel_key)) { event.preventDefault(); cancelTask(); }
  });
  all('.prompt-starters button').forEach((button) => button.addEventListener('click', () => {
    byId('prompt').value = button.dataset.prompt || '';
    byId('prompt').focus(); updateTaskUI();
  }));
  all('.inspector-tabs button').forEach((button) => {
    button.addEventListener('click', () => showPanel(button.dataset.panel));
    button.addEventListener('keydown', (event) => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const tabs = all('.inspector-tabs button');
      const index = tabs.indexOf(button);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
      showPanel(tabs[next].dataset.panel, true);
    });
  });
  window.addEventListener('beforeunload', () => { if (state.stream) state.stream.close(); if (state.liveStream) state.liveStream.close(); clearTimeout(state.pollTimer); });
  window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => { if (state.ui.theme === 'system') applyUiConfig(state.ui); });
  showPanel('trace');
  bootstrap().catch((error) => { setConnection(false); showError('运行时初始化失败：' + errorMessage(error)); });
})();
