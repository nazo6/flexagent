import { For } from "solid-js";
import { cn } from "#lib/utils.ts";

export interface DiffTextProps {
  diff: string;
  class?: string;
  maxHeight?: string;
}

/** Plain (unhighlighted) unified diff; the fallback when shiki is unavailable. */
export default function DiffText(props: DiffTextProps) {
  const lines = () => props.diff.split("\n");
  const maxHeight = () => props.maxHeight ?? "16rem";

  const lineClass = (line: string): string => {
    if (line.startsWith("+++") || line.startsWith("---")) {
      return "text-muted-foreground";
    }
    if (line.startsWith("@@")) return "text-sky-500";
    if (line.startsWith("+")) return "bg-ok-soft/50 text-ok";
    if (line.startsWith("-")) return "bg-danger-soft/50 text-danger";
    return "";
  };

  return (
    <pre
      class={cn(
        "bg-muted/30 overflow-auto rounded-md border p-2 font-mono text-[13px] leading-relaxed",
        props.class ?? "",
      )}
      style={{ "max-height": maxHeight() }}
    >
      <For each={lines()} keyed={false}>
        {(line) => <span class={lineClass(line())}>{`${line()}\n`}</span>}
      </For>
    </pre>
  );
}
