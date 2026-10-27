import { useMemo, useState } from "react";
import { useRouteLoaderData } from "react-router";
import { Search } from "lucide-react";
import type { AppData } from "@/App";
import { Badge, Card, EmptyState, Input } from "@/components/ui";
import { SectionToolbar } from "@/components/shared/bits";

export default function ModelsPage() {
  const { data } = useRouteLoaderData("shell") as { data: AppData };
  const [query, setQuery] = useState("");
  const filtered = useMemo(
    () => data.models.filter((model) => model.id.toLowerCase().includes(query.toLowerCase())),
    [data.models, query],
  );
  return (
    <div className="grid gap-5 pt-7">
      <SectionToolbar
        title="Catalog / exposed models"
        detail="A flat, addressable map for every agent and protocol."
        action={
          <div className="flex w-56 items-center gap-2 border border-border bg-secondary px-2.5 text-muted-foreground focus-within:border-ring">
            <Search className="size-3.5 shrink-0" />
            <Input
              className="h-8 border-0 bg-transparent px-0 focus-visible:border-0 focus-visible:ring-0"
              placeholder="Filter models"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        }
      />
      <Card>
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <strong className="text-[15px] font-semibold tracking-[-0.03em]">
            {filtered.length.toString().padStart(2, "0")} exposed models
          </strong>
          <span className="eyebrow">provider/model</span>
        </div>
        {filtered.length ? (
          <div className="overflow-auto">
            <div className="grid grid-cols-[minmax(250px,1.5fr)_minmax(150px,1fr)_120px_130px] gap-3.5 border-b border-border px-5 py-3">
              <span className="eyebrow">Model id</span>
              <span className="eyebrow">Owner</span>
              <span className="eyebrow">State</span>
              <span className="eyebrow">Type</span>
            </div>
            {filtered.map((model) => (
              <div
                key={model.id}
                className="grid min-h-[52px] grid-cols-[minmax(250px,1.5fr)_minmax(150px,1fr)_120px_130px] items-center gap-3.5 border-b border-border/70 px-5 py-3 last:border-b-0 max-[700px]:grid-cols-2"
              >
                <div className="model-name">
                  <span className="model-bullet" />
                  <strong>{model.id}</strong>
                </div>
                <span className="font-mono text-[10px] text-muted-foreground">{model.provider_name}</span>
                <span className={`font-mono text-[10px] ${model.enabled ? "state-on" : "state-off"}`}>
                  {model.enabled ? "AVAILABLE" : "PAUSED"}
                </span>
                <Badge variant={model.group ? "primary" : "default"}>
                  {model.group ? "ROUTING GROUP" : "UPSTREAM"}
                </Badge>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState title="No matching models" detail="Try another filter or expose a model from a provider." />
        )}
      </Card>
    </div>
  );
}
