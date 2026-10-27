import * as React from "react";
import * as LabelPrimitive from "@radix-ui/react-label";
import { cn } from "@/lib/utils";

function Label({ className, ...props }: React.ComponentProps<typeof LabelPrimitive.Root>) {
  return (
    <LabelPrimitive.Root
      className={cn(
        "font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground select-none",
        className,
      )}
      {...props}
    />
  );
}

export { Label };
