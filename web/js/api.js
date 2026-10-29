// API 客户端：JSON fetch + SSE 封装。所有路径自动带上工作区前缀。

let workspacePrefix = location.pathname.match(/^\/w\/[^/]+(?=\/|$)/)?.[0] || '';
export const getWorkspacePrefix = () => workspacePrefix;
export function setWorkspacePrefix(prefix) {
  workspacePrefix = prefix || '';
}
export const apiPath = (path) => workspacePrefix + path;
// 绝对路径请求：跨工作区资源（如全局工作区列表）不走当前前缀。
export const apiRootPath = (path) => path;
export async function apiRoot(path, options = {}) {
  const headers = { Accept: 'application/json', ...(options.headers || {}) };
  const response = await fetch(path, { ...options, headers });
  const raw = await response.text();
  let body = null;
  if (raw) { try { body = JSON.parse(raw); } catch (_) { body = raw; } }
  if (!response.ok) {
    const detail = body && typeof body === 'object' ? body.error || body.message : body;
    const error = new Error(detail ? String(detail) : `${response.status} ${response.statusText}`);
    error.status = response.status;
    throw error;
  }
  return body;
}

export async function api(path, options = {}) {
  const headers = { Accept: 'application/json', ...(options.headers || {}) };
  const response = await fetch(apiPath(path), { ...options, headers });
  const raw = await response.text();
  let body = null;
  if (raw) { try { body = JSON.parse(raw); } catch (_) { body = raw; } }
  if (!response.ok) {
    const detail = body && typeof body === 'object' ? body.error || body.message : body;
    const error = new Error(detail ? String(detail) : `${response.status} ${response.statusText}`);
    error.status = response.status;
    throw error;
  }
  return body;
}

export function apiPost(path, payload) {
  return api(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload ?? {}),
  });
}

export function apiPatch(path, payload) {
  return api(path, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload ?? {}),
  });
}

export function apiDelete(path) {
  return api(path, { method: 'DELETE' });
}

export function openSse(path, { onTrace, onProgress, onGap, onOpen, onError }) {
  const source = new EventSource(apiPath(path));
  if (onOpen) source.onopen = () => onOpen(source);
  source.onerror = () => onError?.(source);
  if (onTrace) source.addEventListener('trace', (event) => {
    try { onTrace(JSON.parse(event.data)); } catch (_) { /* 跳过损坏帧 */ }
  });
  if (onProgress) source.addEventListener('progress', (event) => {
    try { onProgress(JSON.parse(event.data)); } catch (_) { /* 跳过损坏帧 */ }
  });
  if (onGap) source.addEventListener('gap', () => onGap());
  return source;
}

export const array = (value, key) => {
  if (Array.isArray(value)) return value;
  if (!value || typeof value !== 'object') return [];
  if (key && Array.isArray(value[key])) return value[key];
  return Array.isArray(value.items) ? value.items : Array.isArray(value.data) ? value.data : [];
};
