import type { ReactNode } from "react";
import { Database } from "lucide-react";

export function EmptyState({ title, detail, action }: { title: string; detail: string; action?: ReactNode }) {
  return (
    <div className="grid min-h-36 place-content-center justify-items-center gap-1.5 px-6 py-8 text-center">
      <div className="mb-1 grid size-8 place-items-center border border-input text-ring">
        <Database className="size-4" />
      </div>
      <strong className="text-xs text-foreground">{title}</strong>
      <span className="max-w-xs text-[11px] text-muted-foreground">{detail}</span>
      {action}
    </div>
  );
}
