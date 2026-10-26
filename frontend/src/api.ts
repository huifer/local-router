import type {
  CallerKey,
  Group,
  ModelItem,
  PresetGroup,
  Provider,
  Settings,
  Status,
  UsageData,
} from "./types";

export async function api<T>(path: string, init: Omit<RequestInit, "body"> & { body?: unknown } = {}): Promise<T> {
  const { body, headers, ...rest } = init;
  const response = await fetch(path, {
    ...rest,
    headers: {
      "content-type": "application/json",
      ...(headers ?? {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
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

export async function saveSettings(settings: Settings) {
  return api<{ ok: boolean }>("/api/settings", { method: "PUT", body: settings });
}
