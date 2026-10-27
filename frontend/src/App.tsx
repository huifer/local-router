import { useCallback, useEffect, useState } from "react";
import { createBrowserRouter, Navigate, RouterProvider, useLocation } from "react-router";
import { getStatus, getGroups, getModels, getProviders } from "./api";
import type { Group, ModelItem, Provider, Status } from "./types";
import { AppShell } from "@/layout/app-shell";
import { Toaster } from "@/components/ui";
import OverviewPage from "@/pages/overview";
import ProvidersPage from "@/pages/providers";
import ModelsPage from "@/pages/models";
import GroupsPage from "@/pages/groups";
import UsagePage from "@/pages/usage";
import SettingsPage from "@/pages/settings";

export interface AppData {
  status: Status;
  providers: Provider[];
  groups: Group[];
  models: ModelItem[];
}

export default function App() {
  const [data, setData] = useState<AppData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(async () => {
    const [status, providers, groups, models] = await Promise.all([
      getStatus(),
      getProviders(),
      getGroups(),
      getModels(),
    ]);
    setData({
      status,
      providers: providers.providers,
      groups: groups.groups,
      models: models.models,
    });
    setError(null);
  }, []);

  useEffect(() => {
    reload().catch((e: Error) => setError(e.message));
  }, [reload]);

  if (error) {
    return (
      <div className="grid min-h-dvh place-content-center justify-items-center gap-3 text-center">
        <div className="eyebrow">local-router</div>
        <p className="max-w-sm text-sm text-muted-foreground">
          The control plane could not reach the gateway: {error}
        </p>
        <a href="/" className="text-xs text-primary hover:underline">
          Retry
        </a>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="grid min-h-dvh place-items-center gap-4">
        <div className="size-10 animate-spin rounded-full border border-input border-t-primary" />
        <span className="eyebrow">initializing local-router control plane</span>
      </div>
    );
  }

  const router = createBrowserRouter([
    {
      id: "shell",
      path: "/",
      // The loader hands the pages the same snapshot the shell has, so any
      // route can read it with useRouteLoaderData("shell").
      loader: () => ({ data, reload }),
      element: <AppShell data={data} reload={reload} />,
      children: [
        { index: true, element: <Navigate to="/overview" replace /> },
        { path: "overview", element: <OverviewPage /> },
        { path: "providers", element: <ProvidersPage /> },
        { path: "models", element: <ModelsPage /> },
        { path: "groups", element: <GroupsPage /> },
        { path: "usage", element: <UsagePage /> },
        { path: "settings", element: <SettingsPage /> },
        { path: "*", element: <Navigate to="/overview" replace /> },
      ],
    },
  ]);

  return (
    <>
      <RouterProvider router={router} />
      <Toaster />
    </>
  );
}
