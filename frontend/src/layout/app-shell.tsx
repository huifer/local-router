import { useEffect, useState, type ReactNode } from "react";
import { NavLink, Outlet, useLocation } from "react-router";
import {
  ArrowUpRight,
  AudioLines,
  Cpu,
  KeyRound,
  LayoutGrid,
  Layers,
  RefreshCw,
  Router as RouterIcon,
  Settings2,
} from "lucide-react";
import type { AppData } from "@/App";
import { Button } from "@/components/ui";
import { useT } from "@/i18n";
import { LOCALES } from "@/i18n/messages";
import { cn } from "@/lib/utils";

const nav = [
  { to: "/overview", key: "nav.overview", icon: LayoutGrid },
  { to: "/providers", key: "nav.providers", icon: Cpu },
  { to: "/models", key: "nav.models", icon: Layers },
  { to: "/groups", key: "nav.groups", icon: AudioLines },
  { to: "/usage", key: "nav.usage", icon: KeyRound },
  { to: "/settings", key: "nav.settings", icon: Settings2 },
] as const;

const titleKeys: Record<string, string> = {
  "/overview": "nav.overview",
  "/providers": "nav.providers",
  "/models": "nav.models",
  "/groups": "nav.groups",
  "/usage": "nav.usage",
  "/settings": "nav.settings",
};

export function AppShell({ data, reload, children }: { data: AppData; reload: () => Promise<void>; children?: ReactNode }) {
  const location = useLocation();
  const [busy, setBusy] = useState(false);
  const { t, locale, setLocale } = useT();
  const title = t(titleKeys[location.pathname] ?? "nav.overview");
  const online = true; // The shell only renders once the first status fetch succeeded.

  useEffect(() => {
    window.scrollTo({ top: 0 });
  }, [location.pathname]);

  const refresh = async () => {
    setBusy(true);
    try {
      await reload();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid min-h-dvh grid-cols-[252px_1fr] bg-[radial-gradient(circle_at_88%_-10%,rgba(59,111,81,0.12),transparent_36%)] max-[900px]:grid-cols-1">
      <aside className="sticky top-0 flex h-dvh flex-col border-r border-sidebar-border bg-sidebar/90 px-4 pb-5 pt-6 max-[900px]:h-auto max-[900px]:border-b max-[900px]:border-r-0">
        <div className="flex items-center gap-3 px-3 pb-8">
          <div className="grid size-8 grid-cols-3 items-end gap-[3px]" aria-hidden="true">
            <span className="h-[40%] rounded-t-[2px] bg-primary opacity-55" />
            <span className="h-[85%] rounded-t-[2px] bg-primary" />
            <span className="h-[62%] rounded-t-[2px] bg-primary opacity-75" />
          </div>
          <div>
            <div className="text-[17px] font-extrabold tracking-[-0.04em]">local-router</div>
            <div className="mt-1 font-mono text-[9px] uppercase tracking-[0.06em] text-muted-foreground/70">
              control plane / {data.status.version}
            </div>
          </div>
        </div>
        <div className="eyebrow px-3 pb-2.5">{t("app.workspace")}</div>
        <nav className="grid gap-0.5 max-[900px]:flex max-[900px]:overflow-x-auto" aria-label="Primary navigation">
          {nav.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={({ isActive }) =>
                cn(
                  "relative flex items-center gap-3 rounded-md border border-transparent px-3 py-2.5 text-[12px] text-muted-foreground transition-colors outline-none",
                  "hover:text-foreground hover:bg-white/[0.025] focus-visible:ring-2 focus-visible:ring-ring/60",
                  "max-[900px]:w-auto max-[900px]:shrink-0",
                  isActive && "border-border bg-card text-foreground",
                )
              }
            >
              {({ isActive }) => (
                <>
                  <item.icon className={cn("size-4", isActive && "text-primary")} />
                  <span>{t(item.key)}</span>
                  {isActive && <i className="absolute inset-y-2 right-[-1px] w-0.5 bg-primary max-[900px]:inset-x-2 max-[900px]:bottom-0.5 max-[900px]:top-auto max-[900px]:h-0.5 max-[900px]:w-auto" />}
                </>
              )}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto border-t border-sidebar-border px-3 pt-4 max-[900px]:hidden">
          <div className="flex items-center gap-2.5">
            <StatusDot active={online} />
            <div>
              <strong className="block text-[11px] font-semibold">{t("app.online")}</strong>
              <span className="mt-1 block font-mono text-[10px] text-muted-foreground/70">
                {data.status.settings.gateway_addr}:{data.status.settings.gateway_port}
              </span>
            </div>
          </div>
          <div className="mt-6 font-mono text-[8px] uppercase leading-7 tracking-[0.13em] text-muted-foreground/60">
            {t("app.localOnly")}
          </div>
          <div className="mt-3 flex items-center gap-1">
            {LOCALES.map((item) => (
              <button
                key={item.id}
                onClick={() => setLocale(item.id)}
                title={item.label}
                className={cn(
                  "cursor-pointer rounded-sm border px-1.5 py-0.5 font-mono text-[9px] uppercase transition-colors",
                  locale === item.id
                    ? "border-primary/40 bg-primary/10 text-primary"
                    : "border-border text-muted-foreground hover:text-foreground",
                )}
              >
                {item.short}
              </button>
            ))}
            <span className="ml-1 font-mono text-[8px] uppercase tracking-[0.1em] text-muted-foreground/50">
              {t("app.language")}
            </span>
          </div>
        </div>
      </aside>
      <main className="min-w-0 px-6 pb-16 max-[900px]:px-4 md:px-10">
        <header className="flex min-h-28 items-end justify-between gap-6 border-b border-border pb-6 max-[900px]:min-h-24">
          <div>
            <div className="eyebrow mb-2.5">
              local-router <span className="mx-2 text-border">/</span> {title.toUpperCase()}
            </div>
            <h1 className="m-0 text-[28px] font-bold leading-none tracking-[-0.05em] max-[900px]:text-2xl">{title}</h1>
          </div>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-2 border border-border bg-secondary px-3 py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground max-[700px]:hidden">
              <StatusDot active={online} />
              <span>{t("app.runtimeReady")}</span>
              <RouterIcon className="size-3.5 opacity-60" />
              <Cpu className="size-3.5 opacity-60" />
            </div>
            <Button variant="secondary" onClick={refresh} disabled={busy} aria-label={t("app.refreshAria")}>
              <RefreshCw className={cn(busy && "animate-spin")} />
              {t("app.refresh")}
            </Button>
            <a
              href="https://github.com/huifer/local-router/blob/master/USAGE.md"
              target="_blank"
              rel="noreferrer"
              className="hidden size-9 items-center justify-center rounded-md border border-input text-muted-foreground transition-colors hover:text-foreground max-[700px]:hidden sm:inline-flex"
              aria-label={t("app.docs")}
            >
              <ArrowUpRight className="size-4" />
            </a>
          </div>
        </header>
        <Outlet context={{ data, reload }} />
        {children}
      </main>
    </div>
  );
}

export function StatusDot({ active = true }: { active?: boolean }) {
  return (
    <span
      aria-label={active ? "active" : "inactive"}
      className={cn(
        "inline-block size-[7px] shrink-0 rounded-full",
        active ? "bg-primary shadow-[0_0_0_3px_rgba(158,240,191,0.08)]" : "bg-muted-foreground/50",
      )}
    />
  );
}
