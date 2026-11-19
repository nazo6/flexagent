import { Button } from "#lib/components/ui/button/index.ts";
import IconArrowUp from "~icons/lucide/arrow-up";
import IconLoaderCircle from "~icons/lucide/loader-circle";
import { Show } from "solid-js";

export interface ComposerSendButtonProps {
  /** Shows a spinner while the prompt is being submitted. */
  busy?: boolean;
  disabled?: boolean;
  /** Tooltip (may explain why the button is disabled). */
  title: string;
  /** Accessible name; defaults to the tooltip. */
  label?: string;
  onClick: () => void;
}

/** Round send button shared by the session and new-session composers. */
export default function ComposerSendButton(props: ComposerSendButtonProps) {
  return (
    <Button
      size="icon-sm"
      class="rounded-full"
      disabled={props.disabled ?? false}
      title={props.title}
      aria-label={props.label ?? props.title}
      onClick={() => props.onClick()}
    >
      <Show when={props.busy ?? false} fallback={<IconArrowUp class="size-4" />}>
        <IconLoaderCircle class="size-4 animate-spin" />
      </Show>
    </Button>
  );
}
