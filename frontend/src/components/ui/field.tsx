import * as React from "react";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

function Field({ label, help, className, children }: {
  label: string;
  help?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={cn("grid gap-1.5", className)}>
      <Label>{label}</Label>
      {children}
      {help && <p className="font-mono text-[10px] text-muted-foreground/70">{help}</p>}
    </div>
  );
}

export { Field };
