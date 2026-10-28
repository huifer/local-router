import { useMemo, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { ArrowDown, ArrowUp, Check, Plus, Search, X } from "lucide-react";
import { toast } from "sonner";
import type { AppData } from "@/App";
import type { Group } from "@/types";
import { api } from "@/api";
import {
  Badge, Button, Card, Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  EmptyState, Field, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@/components/ui";
import { SectionToolbar, StatusDot } from "@/components/shared/bits";
import { useT } from "@/i18n";

const routingOptions = [
  { value: "order", label: "order", detailKey: "groups.routingOrderDetail" },
  { value: "rotate", label: "rotate", detailKey: "groups.routingRotateDetail" },
  { value: "usage", label: "usage", detailKey: "groups.routingUsageDetail" },
  { value: "smart", label: "smart", detailKey: "groups.routingSmartDetail" },
  { value: "weight", label: "weight", detailKey: "groups.routingWeightDetail" },
];

const staysOptions = [
  { value: "auto", label: "auto", detailKey: "groups.staysAutoDetail" },
  { value: "session", label: "session", detailKey: "groups.staysSessionDetail" },
  { value: "turn", label: "turn", detailKey: "groups.staysTurnDetail" },
  { value: "off", label: "off", detailKey: "groups.staysOffDetail" },
  { value: "strict", label: "strict", detailKey: "groups.staysStrictDetail" },
];

/** The members a group can be built from: every exposed upstream model. */
function useMemberCatalog() {
  const { data } = useRouteLoaderData("shell") as { data: AppData };
  return useMemo(
    () => data.models.filter((model) => !model.group),
    [data.models],
  );
}

export default function GroupsPage() {
  const { t } = useT();
  const { data, reload } = useRouteLoaderData("shell") as { data: AppData; reload: () => Promise<void> };
  const [showAdd, setShowAdd] = useState(false);
  const [form, setForm] = useState({ name: "", members: [] as string[], routing: "order", stays: "auto" });
  const catalog = useMemberCatalog();
  // The id the group will be reachable under: the name as written, lowercased
  // and trimmed to what a path segment can hold.
  const groupId = useMemo(
    () =>
      form.name
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9._-]+/g, "-")
        .replace(/^-+|-+$/g, "") || "group",
    [form.name],
  );

  const create = async () => {
    if (!form.members.length) {
      toast.error(t("groups.needMember"));
      return;
    }
    try {
      await api("/api/groups", {
        method: "POST",
        body: {
          name: form.name,
          models: form.members,
          routing: form.routing,
          stays: form.stays,
          listed: true,
        },
      });
      toast.success(t("groups.added"));
      setShowAdd(false);
      setForm({ name: "", members: [], routing: "order", stays: "auto" });
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };
  const remove = async (group: Group) => {
    if (!window.confirm(t("groups.removeConfirm", { name: group.name }))) return;
    try {
      await api(`/api/groups/${encodeURIComponent(group.id)}`, { method: "DELETE" });
      toast.success(t("groups.removed"));
      await reload();
    } catch (error) {
      toast.error((error as Error).message);
    }
  };

  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title={t("groups.eyebrow")}
        detail={t("groups.detail")}
        action={
          <Button variant="default" onClick={() => setShowAdd(true)} disabled={!data.models.length}>
            <Plus /> {t("groups.add")}
          </Button>
        }
      />

      {!data.models.length ? (
        <Card>
          <EmptyState
            title={t("groups.emptyNoModels")}
            detail={t("groups.emptyNoModelsDetail")}
          />
        </Card>
      ) : data.groups.length ? (
        <div className="grid grid-cols-2 gap-3 max-[900px]:grid-cols-1">
          {data.groups.map((group) => (
            <Card key={group.id} className="p-4.5">
              <div className="flex items-start justify-between gap-4">
                <div>
                  <span className="font-mono text-[10px] text-ring">group/{group.id}</span>
                  <h3 className="m-0 mt-2 text-lg tracking-[-0.04em]">{group.name}</h3>
                </div>
                <Badge variant="primary">{group.routing}</Badge>
              </div>
              <div className="my-5 mb-3 flex items-center gap-2">
                <span className="eyebrow">{t("groups.stays")}</span>
                <strong className="font-mono text-[11px]">{group.stays}</strong>
                <span className="eyebrow ml-auto">{t("groups.members")}</span>
                <strong className="font-mono text-[11px]">{group.models.length.toString().padStart(2, "0")}</strong>
              </div>
              <div className="border-t border-border">
                {group.models.map((model, index) => {
                  const owner = model.split("/")[0];
                  const last = index === group.models.length - 1;
                  return (
                    <div key={model} className="flex items-center gap-2.5 py-2.5 font-mono text-[11px]">
                      <span className="relative grid w-5 shrink-0 place-items-center self-stretch">
                        <span
                          className="absolute left-1/2 top-0 h-full w-px -translate-x-1/2 bg-border"
                          style={{ display: last ? "none" : undefined }}
                        />
                        <span
                          className={`relative grid size-4 place-items-center border text-[8px] ${
                            index === 0
                              ? "border-primary/50 bg-primary/10 text-primary"
                              : "border-border bg-secondary text-muted-foreground"
                          }`}
                        >
                          {index + 1}
                        </span>
                      </span>
                      <span className="truncate">{model}</span>
                      <Badge className="ml-auto shrink-0">{owner}</Badge>
                      {index === 0 && group.routing === "order" && (
                        <em className="shrink-0 font-mono text-[9px] not-italic text-ring">{t("groups.primary")}</em>
                      )}
                    </div>
                  );
                })}
              </div>
              <div className="mt-3 flex items-center justify-between border-t border-border pt-3">
                <span className="flex items-center gap-2 font-mono text-[10px] text-muted-foreground">
                  <StatusDot /> {t("groups.routeReady")}
                </span>
                <Button variant="destructive" size="sm" onClick={() => remove(group)}>{t("groups.remove")}</Button>
              </div>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <EmptyState
            title={t("groups.empty")}
            detail={t("groups.emptyDetail")}
          />
        </Card>
      )}

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent className="flex h-[min(86vh,780px)] max-w-4xl flex-col overflow-hidden p-0">
          <DialogHeader className="shrink-0 border-b border-border px-5 py-4">
            <DialogTitle>{t("groups.create")}</DialogTitle>
            <p className="mt-1 font-mono text-[10px] text-muted-foreground/70">
              {t("groups.createDetail")}
            </p>
          </DialogHeader>

          <div className="flex min-h-0 flex-1 max-[860px]:flex-col">
            {/* Left: the model catalog. */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col border-r border-border max-[860px]:border-b max-[860px]:border-r-0">
              <div className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2.5">
                <span className="eyebrow">{t("groups.catalog")}</span>
                <span className="ml-auto font-mono text-[9px] text-muted-foreground/60">
                  {t("groups.modelsCount", { n: catalog.length })}
                </span>
              </div>
              <MemberPicker
                selected={form.members}
                onChange={(members) => setForm({ ...form, members })}
              />
            </div>

            {/* Right: the route being built. */}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <div className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2.5">
                <span className="eyebrow">{t("groups.routePreview")}</span>
                <span className="ml-auto font-mono text-[9px] text-muted-foreground/60">
                  {t("groups.selectedCountPlural", { n: form.members.length })}
                </span>
              </div>
              <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
                <RouteChain
                  selected={form.members}
                  onChange={(members) => setForm({ ...form, members })}
                />
              </div>
            </div>
          </div>

          {/* The policy: how the chain is walked. */}
          <div className="grid shrink-0 gap-4 border-t border-border px-5 py-4">
            <Field label={t("groups.routing")} help={t("groups.routingHelp")}>
              <div className="grid grid-cols-5 gap-1.5 max-[720px]:grid-cols-2">
                {routingOptions.map((option) => {
                  const active = form.routing === option.value;
                  return (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => setForm({ ...form, routing: option.value })}
                      className={`cursor-pointer border px-2.5 py-2 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/60 ${
                        active
                          ? "border-primary/40 bg-primary/5"
                          : "border-border bg-secondary/40 hover:border-input hover:bg-accent"
                      }`}
                    >
                      <span className={`block font-mono text-[11px] ${active ? "text-primary" : "text-foreground"}`}>
                        {option.label}
                      </span>
                      <span className="mt-1 block text-[10px] leading-snug text-muted-foreground/80">
                        {t(option.detailKey)}
                      </span>
                    </button>
                  );
                })}
              </div>
            </Field>

            <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-4 max-[560px]:grid-cols-1">
              <Field label={t("groups.groupName")}>
                <Input
                  value={form.name}
                  placeholder={t("groups.groupNamePlaceholder")}
                  onChange={(event) => setForm({ ...form, name: event.target.value })}
                />
              </Field>
              <Field label={t("groups.affinity")} help={t("groups.affinityHelp")}>
                <Select value={form.stays} onValueChange={(value) => setForm({ ...form, stays: value })}>
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    {staysOptions.map((option) => (
                      <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="font-mono text-[10px] text-muted-foreground/70">
                  {t(staysOptions.find((option) => option.value === form.stays)?.detailKey ?? "groups.staysAutoDetail")}
                </p>
              </Field>
            </div>
          </div>

          <DialogFooter className="shrink-0 border-t border-border px-5 py-3.5">
            <span className="mr-auto flex min-w-0 items-center gap-2 font-mono text-[10px] text-muted-foreground">
              <span className="shrink-0 text-ring">group/{groupId}</span>
              <span className="truncate">
                {form.members.length
                  ? form.members.join(" → ")
                  : t("groups.previewEmpty")}
              </span>
            </span>
            <Button variant="secondary" onClick={() => setShowAdd(false)}>{t("groups.cancel")}</Button>
            <Button variant="default" onClick={create} disabled={!form.members.length}>
              {t("groups.createButton")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/**
 * The route being built: the members in the order they are tried. It is a
 * chain, so it is drawn as one — the first member answers, the rest are
 * failover — and the order can be changed here.
 */
function RouteChain({ selected, onChange }: { selected: string[]; onChange: (next: string[]) => void }) {
  const { t } = useT();

  const move = (index: number, delta: number) => {
    const next = [...selected];
    const target = index + delta;
    if (target < 0 || target >= next.length) return;
    [next[index], next[target]] = [next[target], next[index]];
    onChange(next);
  };

  const remove = (id: string) => onChange(selected.filter((item) => item !== id));

  if (!selected.length) {
    return (
      <div className="grid place-items-center border border-dashed border-border px-4 py-10 text-center">
        <div>
          <div className="font-mono text-[11px] text-muted-foreground/70">{t("groups.chainEmpty")}</div>
          <div className="mt-1.5 font-mono text-[10px] text-muted-foreground/40">
            {t("groups.chainEmptyDetail")}
          </div>
        </div>
      </div>
    );
  }

  return (
    <ol className="grid gap-0">
      {selected.map((id, index) => (
        <li key={id} className="relative flex items-center gap-3 pb-2.5 pl-0 last:pb-0">
          {/* The chain: a rail down the left with a node per member. */}
          <span className="relative grid w-6 shrink-0 place-items-center self-stretch">
            <span className="absolute left-1/2 top-0 h-full w-px -translate-x-1/2 bg-ring/25" style={{ display: index === selected.length - 1 ? "none" : undefined }} />
            <span
              className={`relative grid size-5 place-items-center border font-mono text-[9px] ${
                index === 0
                  ? "border-primary/50 bg-primary/10 text-primary"
                  : "border-border bg-secondary text-muted-foreground"
              }`}
            >
              {index + 1}
            </span>
          </span>

          <span className="min-w-0 flex-1 border border-border bg-secondary/40 px-2.5 py-1.5">
            <span className="flex items-center gap-2">
              <span className="truncate font-mono text-[11px] text-foreground">{id}</span>
              {index === 0 ? <Badge variant="primary">{t("groups.primary")}</Badge> : null}
            </span>
            <span className="mt-0.5 block font-mono text-[9px] text-muted-foreground/60">
              {id.split("/")[0]}
            </span>
          </span>

          <span className="flex shrink-0 items-center gap-0.5">
            <button
              type="button"
              onClick={() => move(index, -1)}
              disabled={index === 0}
              title={t("groups.moveUp")}
              className="grid size-6 cursor-pointer place-items-center border border-border text-muted-foreground transition-colors hover:border-input hover:text-foreground disabled:cursor-not-allowed disabled:opacity-30"
            >
              <ArrowUp className="size-3" />
            </button>
            <button
              type="button"
              onClick={() => move(index, 1)}
              disabled={index === selected.length - 1}
              title={t("groups.moveDown")}
              className="grid size-6 cursor-pointer place-items-center border border-border text-muted-foreground transition-colors hover:border-input hover:text-foreground disabled:cursor-not-allowed disabled:opacity-30"
            >
              <ArrowDown className="size-3" />
            </button>
            <button
              type="button"
              onClick={() => remove(id)}
              title={t("groups.removeMemberHint")}
              className="grid size-6 cursor-pointer place-items-center border border-transparent text-muted-foreground transition-colors hover:border-destructive/30 hover:bg-destructive/10 hover:text-destructive"
            >
              <X className="size-3" />
            </button>
          </span>
        </li>
      ))}
    </ol>
  );
}

/**
 * The member picker: the models the providers expose, grouped by provider,
 * with a search box. Picking a model adds it to the end of the chain.
 */
function MemberPicker({ selected, onChange }: { selected: string[]; onChange: (next: string[]) => void }) {
  const { t } = useT();
  const catalog = useMemberCatalog();
  const [query, setQuery] = useState("");

  const byProvider = useMemo(() => {
    const map = new Map<string, { id: string; label: string }[]>();
    for (const model of catalog) {
      const list = map.get(model.provider) ?? [];
      list.push({ id: model.id, label: model.model });
      map.set(model.provider, list);
    }
    return [...map.entries()];
  }, [catalog]);

  const matches = (model: { id: string; label: string }) =>
    !query.trim() ||
    model.id.toLowerCase().includes(query.toLowerCase()) ||
    model.label.toLowerCase().includes(query.toLowerCase());

  const toggle = (id: string) => {
    onChange(selected.includes(id) ? selected.filter((item) => item !== id) : [...selected, id]);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-[#0c1110] px-3 py-2 text-muted-foreground focus-within:border-ring">
        <Search className="size-3.5 shrink-0" />
        <Input
          className="h-7 border-0 bg-transparent px-0 focus-visible:ring-0"
          placeholder={t("groups.searchModels")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {byProvider.map(([provider, models]) => {
          const visible = models.filter(matches);
          if (!visible.length) return null;
          return (
            <div key={provider}>
              <div className="sticky top-0 z-10 flex items-center gap-2 border-b border-border bg-secondary/90 px-3 py-1.5 backdrop-blur">
                <span className="eyebrow">{provider}</span>
                <span className="font-mono text-[9px] text-muted-foreground/60">{t("groups.modelsCount", { n: visible.length })}</span>
              </div>
              {visible.map((model) => {
                const index = selected.indexOf(model.id);
                const picked = index !== -1;
                return (
                  <button
                    key={model.id}
                    onClick={() => toggle(model.id)}
                    className={`flex w-full cursor-pointer items-center gap-2.5 border-b border-border/40 px-3 py-1.5 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60 ${
                      picked ? "bg-primary/5 text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"
                    }`}
                  >
                    <span
                      className={`grid size-4 shrink-0 place-items-center border ${
                        picked ? "border-primary bg-primary text-primary-foreground" : "border-input"
                      }`}
                    >
                      {picked && <Check className="size-3" />}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-[11px]">{model.label}</span>
                    {picked ? <Badge variant="primary">{index + 1}</Badge> : null}
                  </button>
                );
              })}
            </div>
          );
        })}
        {!catalog.length && (
          <p className="px-3 py-8 text-center font-mono text-[10px] text-muted-foreground/70">
            {t("groups.noModelsExposed")}
          </p>
        )}
      </div>
    </div>
  );
}
