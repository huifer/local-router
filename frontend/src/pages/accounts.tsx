import { useEffect, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { LogIn, RefreshCw, UserRound } from "lucide-react";
import { toast } from "sonner";
import type { AccountView, Provider } from "@/types";
import type { AppData } from "@/App";
import { api } from "@/api";
import { Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, EmptyState } from "@/components/ui";
import { SectionToolbar, StatusDot, formatTime } from "@/components/shared/bits";

export default function AccountsPage() {
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const [loading, setLoading] = useState(true);

  const fetchAccounts = async () => {
    setLoading(true);
    try {
      const response = await api<{ accounts: AccountView[] }>("/api/accounts");
      setAccounts(response.accounts);
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { fetchAccounts(); }, []);

  const sessionProviders = data.providers.filter((provider) => provider.auth);

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title="Accounts / sign-in state"
        detail="See which vendor accounts are signed in, their plan, balance and token expiry."
        action={<Button variant="ghost" onClick={fetchAccounts}><RefreshCw className={loading ? "animate-spin" : ""} /> Refresh</Button>}
      />
      {!accounts.length && !loading ? (
        <Card><EmptyState title="No login accounts yet" detail="Open Providers → Add provider and choose a vendor with the LOGIN or OAUTH badge." /></Card>
      ) : (
        <div className="grid grid-cols-2 gap-3 max-[900px]:grid-cols-1">
          {accounts.map((account) => <AccountCard key={`${account.provider}-${account.vendor}`} account={account} providers={sessionProviders} onReload={async () => { await fetchAccounts(); await reload(); }} />)}
        </div>
      )}
    </div>
  );
}

function AccountCard({ account, providers, onReload }: { account: AccountView; providers: Provider[]; onReload: () => Promise<void> }) {
  void providers;
  const provider = providers.find((item) => item.id === account.provider);
  const [busy, setBusy] = useState(false);
  const login = async () => {
    setBusy(true);
    try {
      const flow = await api<{ url: string }>(`/api/providers/${encodeURIComponent(account.provider)}/login`, { method: "POST" });
      window.open(flow.url, "_blank", "noopener");
      toast.success("Sign-in page opened. Finish it in the browser, then wait here.");
      const deadline = Date.now() + 5 * 60 * 1000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 2000));
        const result = await api<{ done: boolean; account?: string }>(`/api/providers/${encodeURIComponent(account.provider)}/login/status`);
        if (result.done) { toast.success(`Signed in${result.account ? ` as ${result.account}` : ""}`); await onReload(); return; }
      }
      toast.error("Sign-in timed out");
    } catch (error) { toast.error((error as Error).message); }
    finally { setBusy(false); }
  };

  return (
    <Card>
      <CardHeader className="flex-row items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="grid size-10 place-items-center border border-primary/25 bg-primary/5 text-primary"><UserRound className="size-5" /></div>
          <div>
            <CardDescription className="eyebrow">{account.vendor}</CardDescription>
            <CardTitle>{account.account?.nickname || account.provider}</CardTitle>
            <p className="mt-1 font-mono text-[10px] text-muted-foreground">{account.account?.uid || account.provider}</p>
          </div>
        </div>
        <Badge variant={account.signed_in ? "primary" : "destructive"}>{account.signed_in ? "SIGNED IN" : "NOT SIGNED IN"}</Badge>
      </CardHeader>
      <CardContent className="grid gap-4">
        {account.signed_in ? (
          <>
            <div className="grid grid-cols-2 gap-3">
              <Info label="Provider" value={account.provider} />
              <Info label="Account type" value={account.account?.type || account.kind} />
              <Info label="Token expires" value={account.token_expires_at ? formatTime(new Date(account.token_expires_at).toISOString()) : "unknown"} />
              <Info label="Plan" value={account.plan || account.balance || "session"} />
            </div>
            {account.credits && <CreditSummary credits={account.credits} onCheckin={onReload} provider={account.provider} />}
            {account.credits?.packages.length ? (
              <div className="overflow-auto">
                <div className="grid grid-cols-[minmax(180px,2fr)_90px_110px_90px] gap-3 border-b border-border px-1 pb-2">
                  <span className="eyebrow">Package</span><span className="eyebrow">Remaining</span><span className="eyebrow">Capacity</span><span className="eyebrow">Used</span>
                </div>
                {account.credits.packages.map((item, index) => (
                  <div key={`${item.package_name}-${index}`} className="grid grid-cols-[minmax(180px,2fr)_90px_110px_90px] items-center gap-3 border-b border-border/60 py-2.5 last:border-b-0">
                    <span className="truncate font-mono text-[10px]">{item.package_name}</span>
                    <span className="font-mono text-[10px]">{item.remaining}</span>
                    <span className="font-mono text-[10px]">{item.capacity} {item.unit}</span>
                    <span className="font-mono text-[10px]">{item.used}</span>
                  </div>
                ))}
              </div>
            ) : null}
            {account.pools?.length ? <div className="flex flex-wrap gap-2">{account.pools.map((pool) => <Badge key={pool.name} variant="primary">{pool.name}: {pool.balance}</Badge>)}</div> : null}
          </>
        ) : (
          <div className="grid gap-3">
            <p className="text-xs text-muted-foreground">No credentials are connected yet. Sign in through the vendor's browser page.</p>
            {account.error && <p className="font-mono text-[10px] text-destructive">{account.error}</p>}
            <Button variant="default" onClick={login} disabled={busy}><LogIn /> {busy ? "Waiting for sign-in…" : "Sign in in browser"}</Button>
          </div>
        )}
        {provider && <div className="flex items-center gap-2 border-t border-border pt-3 font-mono text-[10px] text-muted-foreground"><StatusDot active={provider.enabled} /> provider/{provider.id} · {provider.protocol}</div>}
      </CardContent>
    </Card>
  );
}

function Info({ label, value }: { label: string; value: string }) {
  return <div className="border border-border bg-secondary/40 p-3"><div className="eyebrow">{label}</div><div className="mt-2 break-all font-mono text-[11px] text-foreground">{value}</div></div>;
}

function CreditSummary({ credits, onCheckin, provider }: { credits: NonNullable<AccountView["credits"]>; onCheckin: () => Promise<void>; provider: string }) {
  const [busy, setBusy] = useState(false);
  const checkin = async () => {
    setBusy(true);
    try {
      const result = await api<{ granted?: boolean; already?: boolean; credit?: number | null; message?: string }>(
        "/api/checkin",
        { method: "POST", body: { provider } },
      );
      if (result.granted) toast.success(`Checked in — +${result.credit ?? 0} credits`);
      else toast.success(result.message || "Already checked in today");
      await onCheckin();
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="grid gap-3">
      <div className="grid grid-cols-3 gap-3">
        <Info label="Credits left" value={`${credits.remaining} / ${credits.capacity}`} />
        <Info label="Packages" value={String(credits.packages.length)} />
        <div className="border border-border bg-secondary/40 p-3">
          <div className="eyebrow">Check-in</div>
          <div className="mt-2 flex items-center justify-between gap-2">
            <span className="font-mono text-[11px]">{credits.checkin?.checked_in_today ? "done" : "pending"}</span>
            <Button variant="secondary" size="sm" onClick={checkin} disabled={busy || credits.checkin?.checked_in_today === true}>
              {busy ? "…" : credits.checkin?.checked_in_today ? "Done" : "Claim"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
