import type {
  CallerKey,
  Group,
  ModelItem,
  PresetGroup,
  ProjectRow,
  Provider,
  Settings,
  Status,
  TraceDetail,
  TraceSessionDetail,
  TraceSessionsData,
  TracesData,
  UsageData,
  WorkBuddyStatus,
} from "./types";

// The console needs no key on loopback. When the page is served from a LAN
// address, /api answers 401 until a key is set — the same key the gateway
// takes (LOCAL_ROUTER_DEFAULT_KEY, the default key, or any caller key).
const WEB_KEY_STORAGE = "local-router.web-key";

function getWebKey(): string {
  try {
    return localStorage.getItem(WEB_KEY_STORAGE) ?? "";
  } catch {
    return "";
  }
}

function rememberWebKey(key: string): void {
  try {
    localStorage.setItem(WEB_KEY_STORAGE, key);
  } catch {
    // Private mode: keep working without persistence.
  }
}

export async function api<T>(
  path: string,
  init: Omit<RequestInit, "body"> & { body?: unknown } = {},
  retried = false,
): Promise<T> {
  const { body, headers, ...rest } = init;
  const key = getWebKey();
  const response = await fetch(path, {
    ...rest,
    headers: {
      "content-type": "application/json",
      ...(key ? { authorization: `Bearer ${key}` } : {}),
      ...(headers ?? {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.status === 401 && !retried) {
    const input = window.prompt(
      "控制台需要访问密钥（LOCAL_ROUTER_DEFAULT_KEY / 默认密钥 / 任一 caller key）：",
    );
    if (input !== null) {
      rememberWebKey(input.trim());
      return api(path, init, true);
    }
  }
  const text = await response.text();
  let data: unknown = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = { error: text };
  }
  if (!response.ok) {
    const message = typeof data === "object" && data && "error" in data
      ? String((data as { error: unknown }).error)
      : `请求失败 (${response.status})`;
    throw new Error(message);
  }
  return data as T;
}

export const getStatus = () => api<{ version: string; providers: number; groups: number; models: number; settings: Settings }>("/api/status");
export const getProviders = () => api<{ providers: Provider[] }>("/api/providers");
export const getGroups = () => api<{ groups: Group[] }>("/api/groups");
export const getModels = () => api<{ models: ModelItem[] }>("/api/models");
export const getPresets = () => api<{ presets: PresetGroup[] }>("/api/presets");
export const getUsage = (period = "7d") => api<UsageData>(`/api/usage?period=${period}`);
export const getTraces = (period = "7d") => api<TracesData>(`/api/traces?period=${period}`);
export const getTrace = (id: string) =>
  api<TraceDetail>(`/api/traces/${encodeURIComponent(id)}`);
/** Every session the gateway has seen, with the whole chain of each. */
export const getTraceSessions = (period = "7d") =>
  api<TraceSessionsData>(`/api/traces/sessions?period=${period}`);
/** One session, with every request and every vendor call it produced. */
export const getTraceSession = (id: string) =>
  api<TraceSessionDetail>(`/api/traces/sessions/${encodeURIComponent(id)}`);
/** The trace log as a file: csv for a spreadsheet, jsonl for a script. */
export async function downloadTraces(format: "csv" | "jsonl", period = "7d") {
  const response = await fetch(
    `/api/traces/export?format=${format}&period=${period}`,
    {
      headers: getWebKey() ? { authorization: `Bearer ${getWebKey()}` } : {},
    },
  );
  if (!response.ok) {
    throw new Error(`the export failed (${response.status})`);
  }
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = format === "jsonl" ? "traces.jsonl" : "traces.csv";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
export const getProjects = (period = "7d") => api<{ projects: ProjectRow[] }>(`/api/projects?period=${period}`);
export const updateProject = (id: string, patch: { name?: string; color?: string; budget_tokens?: number | null; budget_cost?: number | null }) =>
  api<{ ok: boolean; projects: ProjectRow[] }>(`/api/projects?id=${encodeURIComponent(id)}`, { method: "PATCH", body: patch });
export const mergeProjects = (from: string, into: string) =>
  api<{ ok: boolean; projects: ProjectRow[] }>("/api/projects/merge", { method: "POST", body: { from, into } });
export const getCallerKeys = () => api<{ keys: CallerKey[] }>("/api/caller-keys");

export const getWorkBuddy = () => api<WorkBuddyStatus>("/api/workbuddy");

export async function saveSettings(settings: Settings) {
  return api<{ ok: boolean }>("/api/settings", { method: "PUT", body: settings });
}
