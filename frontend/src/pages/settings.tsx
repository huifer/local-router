import { useEffect, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { CallerKey, Settings } from "@/types";
import { api, getCallerKeys, saveSettings } from "@/api";
import {
  Button, Card, CardContent, CardDescription, CardHeader, CardTitle, Field, Input, Switch,
} from "@/components/ui";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";
import { useT } from "@/i18n";
import { LOCALES } from "@/i18n/messages";
import { cn } from "@/lib/utils";

export default function SettingsPage() {
  const { t, locale, setLocale } = useT();
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [form, setForm] = useState<Settings>(data.status.settings);
  const [callerKeys, setCallerKeys] = useState<CallerKey[]>([]);

  useEffect(() => {
    getCallerKeys().then((resp) => setCallerKeys(resp.keys)).catch(() => {});
  }, []);

  /** Switch the language and keep it in the gateway's settings. */
  const pickLocale = async (next: "en" | "zh-CN") => {
    setLocale(next);
    const updated = { ...form, locale: next };
    setForm(updated);
    try {
      await saveSettings(updated);
      toast.success(t("settings.languageSaved"));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const save = async () => {
    try {
      await saveSettings(form);
      toast.success(t("settings.saved"));
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const addKey = async () => {
    const name = window.prompt(t("settings.nameKey"));
    if (!name) return;
    try {
      await api("/api/caller-keys", { method: "POST", body: { name } });
      const resp = await getCallerKeys();
      setCallerKeys(resp.keys);
      toast.success(t("settings.callerKeyCreated"));
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title={t("settings.eyebrow")}
        detail={t("settings.detail")}
        action={<Button variant="default" onClick={save}>{t("settings.save")}</Button>}
      />
      <div className="grid grid-cols-2 gap-5 max-[900px]:grid-cols-1">
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("settings.network")}</CardDescription>
            <CardTitle>{t("settings.whereListens")}</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <div className="grid grid-cols-2 gap-3.5 max-[500px]:grid-cols-1">
              <Field label={t("settings.gatewayAddr")}>
                <Input value={form.gateway_addr} onChange={(event) => setForm({ ...form, gateway_addr: event.target.value })} />
              </Field>
              <Field label={t("settings.gatewayPort")} help={t("settings.gatewayPortHelp")}>
                <Input type="number" value={form.gateway_port} onChange={(event) => setForm({ ...form, gateway_port: Number(event.target.value) })} />
              </Field>
            </div>
            <SettingRow label={t("settings.shareLan")} detail={t("settings.shareLanDetail")}>
              <Switch checked={form.lan} onCheckedChange={(checked) => setForm({ ...form, lan: checked })} />
            </SettingRow>
            <SettingRow label={t("settings.sendUa")} detail={t("settings.sendUaDetail")}>
              <Switch checked={form.send_agent_user_agent} onCheckedChange={(checked) => setForm({ ...form, send_agent_user_agent: checked })} />
            </SettingRow>
            <SettingRow label={t("settings.recordUsage")} detail={t("settings.recordUsageDetail")}>
              <Switch checked={form.record_usage} onCheckedChange={(checked) => setForm({ ...form, record_usage: checked })} />
            </SettingRow>
            <SettingRow label={t("settings.maskSecrets")} detail={t("settings.maskSecretsDetail")}>
              <Switch checked={form.masking} onCheckedChange={(checked) => setForm({ ...form, masking: checked })} />
            </SettingRow>
            <SettingRow label={t("settings.autoCheckin")} detail={t("settings.autoCheckinDetail")}>
              <Switch checked={form.workbuddy_auto_checkin} onCheckedChange={(checked) => setForm({ ...form, workbuddy_auto_checkin: checked })} />
            </SettingRow>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="eyebrow">{t("settings.callerAccess")}</CardDescription>
            <CardTitle>{t("settings.keysOtherMachines")}</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <Field label={t("settings.defaultKey")} help={t("settings.defaultKeyHelp")}>
              <Input value={form.default_caller_key} onChange={(event) => setForm({ ...form, default_caller_key: event.target.value })} />
            </Field>
            {callerKeys.length ? (
              <div className="border border-border">
                {callerKeys.map((key) => (
                  <div key={key.id} className="flex items-center justify-between border-b border-border p-3 last:border-b-0">
                    <div>
                      <strong className="block text-[11px]">{key.name}</strong>
                      <span className="mt-0.5 block font-mono text-[10px] text-muted-foreground">{key.key}</span>
                    </div>
                    <StatusDot active={key.enabled} />
                  </div>
                ))}
              </div>
            ) : (
              <p className="font-mono text-[10px] text-muted-foreground/70">
                {t("settings.noCallerKeys")}
              </p>
            )}
            <Button variant="secondary" onClick={addKey}>
              <Plus /> {t("settings.createCallerKey")}
            </Button>
            <div className="flex items-center justify-between gap-4 border-t border-border pt-3.5">
              <div>
                <div className="text-xs text-foreground">{t("app.language")}</div>
                <p className="mt-0.5 text-[10px] text-muted-foreground">{t("settings.languageDetail")}</p>
              </div>
              <div className="flex gap-1.5">
                {LOCALES.map((item) => (
                  <button
                    key={item.id}
                    onClick={() => pickLocale(item.id)}
                    className={cn(
                      "cursor-pointer rounded-sm border px-2.5 py-1 font-mono text-[10px] uppercase transition-colors",
                      locale === item.id
                        ? "border-primary/40 bg-primary/10 text-primary"
                        : "border-border text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {item.short}
                  </button>
                ))}
              </div>
            </div>
            <Field label={t("settings.corsOrigins")} help={t("settings.corsHelp")}>
              <Input
                value={(form.cors_origins ?? []).join(", ")}
                placeholder='["http://localhost:3000"]'
                onChange={(event) =>
                  setForm({
                    ...form,
                    cors_origins: event.target.value.split(",").map((item) => item.trim()).filter(Boolean),
                  })
                }
              />
            </Field>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function SettingRow({ label, detail, children }: { label: string; detail: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-border pt-3.5 first:border-t-0 first:pt-0">
      <div>
        <div className="text-xs text-foreground">{label}</div>
        <p className="mt-0.5 text-[10px] text-muted-foreground">{detail}</p>
      </div>
      {children}
    </div>
  );
}
