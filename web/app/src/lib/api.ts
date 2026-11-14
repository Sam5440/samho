// API 客户端：JSON fetch + SSE 封装。所有路径自动带上工作区前缀（/w/{id}）。

let workspacePrefix = location.pathname.match(/^\/w\/[^/]+(?=\/|$)/)?.[0] || '';
export const getWorkspacePrefix = () => workspacePrefix;
export function setWorkspacePrefix(prefix: string) {
  workspacePrefix = prefix || '';
}
export const apiPath = (path: string) => workspacePrefix + path;

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

async function request<T>(url: string, options: RequestInit = {}): Promise<T> {
  const headers = { Accept: 'application/json', ...(options.headers || {}) };
  const response = await fetch(url, { ...options, headers });
  const raw = await response.text();
  let body: unknown = null;
  if (raw) { try { body = JSON.parse(raw); } catch { body = raw; } }
  if (!response.ok) {
    const detail = body && typeof body === 'object'
      ? (body as Record<string, unknown>).error || (body as Record<string, unknown>).message
      : body;
    throw new ApiError(detail ? String(detail) : `${response.status} ${response.statusText}`, response.status);
  }
  return body as T;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

export const api = <T = Json>(path: string, options?: RequestInit) => request<T>(apiPath(path), options);
// 绝对路径请求：跨工作区资源（全局工作区列表、设备目录浏览）不走当前前缀。
export const apiRoot = <T = Json>(path: string, options?: RequestInit) => request<T>(path, options);

const withBody = (method: string, payload: unknown): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(payload ?? {}),
});

export const apiPost = <T = Json>(path: string, payload?: unknown) => api<T>(path, withBody('POST', payload));
export const apiPatch = <T = Json>(path: string, payload?: unknown) => api<T>(path, withBody('PATCH', payload));
export const apiPut = <T = Json>(path: string, payload?: unknown) => api<T>(path, withBody('PUT', payload));
export const apiDelete = <T = Json>(path: string) => api<T>(path, { method: 'DELETE' });
export const apiRootSend = <T = Json>(path: string, method: string, payload?: unknown) =>
  apiRoot<T>(path, payload === undefined ? { method } : withBody(method, payload));

export function openSse(path: string, handlers: {
  onTrace?: (event: Json) => void;
  onProgress?: (progress: Json) => void;
  onGap?: () => void;
  onOpen?: () => void;
  onError?: () => void;
}) {
  const source = new EventSource(apiPath(path));
  if (handlers.onOpen) source.onopen = () => handlers.onOpen?.();
  source.onerror = () => handlers.onError?.();
  if (handlers.onTrace) source.addEventListener('trace', (event) => {
    try { handlers.onTrace?.(JSON.parse((event as MessageEvent).data)); } catch { /* 跳过损坏帧 */ }
  });
  if (handlers.onProgress) source.addEventListener('progress', (event) => {
    try { handlers.onProgress?.(JSON.parse((event as MessageEvent).data)); } catch { /* 跳过损坏帧 */ }
  });
  if (handlers.onGap) source.addEventListener('gap', () => handlers.onGap?.());
  return source;
}

export function array<T = Json>(value: unknown, key?: string): T[] {
  if (Array.isArray(value)) return value as T[];
  if (!value || typeof value !== 'object') return [];
  const record = value as Record<string, unknown>;
  if (key && Array.isArray(record[key])) return record[key] as T[];
  if (Array.isArray(record.items)) return record.items as T[];
  if (Array.isArray(record.data)) return record.data as T[];
  return [];
}

// 从 location 推导当前工作区 id（无前缀 → default）。
export function workspaceIdFromLocation() {
  const match = location.pathname.match(/^\/w\/([^/]+)/);
  return match ? decodeURIComponent(match[1]) : 'default';
}
