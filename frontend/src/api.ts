import type {
  CallerKey,
  Group,
  ModelItem,
  PresetGroup,
  Provider,
  Settings,
  Status,
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
export const getUsage = () => api<UsageData>("/api/usage");
export const getCallerKeys = () => api<{ keys: CallerKey[] }>("/api/caller-keys");

export const getWorkBuddy = () => api<WorkBuddyStatus>("/api/workbuddy");

export async function saveSettings(settings: Settings) {
  return api<{ ok: boolean }>("/api/settings", { method: "PUT", body: settings });
}
