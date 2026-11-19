import { Show } from "solid-js";
import { errorMessage } from "#lib/api/error-messages.ts";
import { Button } from "#lib/components/ui/button/index.ts";
import { useDocumentTitle } from "#lib/document-title.ts";
import { m } from "#lib/paraglide/messages.js";
import { cn } from "#lib/utils.ts";
import IconTriangleAlert from "~icons/lucide/triangle-alert";

export interface ErrorPageProps {
  /** 404 (unknown route) instead of an unexpected render failure. */
  notFound?: boolean;
  /** Failure shown under the technical details. */
  error?: unknown;
  class?: string;
}

function describeError(error: unknown): string | null {
  if (error === undefined || error === null) return null;
  return errorMessage(error);
}

/**
 * Error screen for unknown routes and uncaught render failures.
 *
 * Ported from the SvelteKit `+error.svelte`: it is deliberately independent of
 * the app shell, so it can replace the whole screen when the router itself
 * throws.
 */
export default function ErrorPage(props: ErrorPageProps) {
  const title = () =>
    props.notFound === true ? m.error_page_not_found_title() : m.error_page_unexpected_title();
  const body = () =>
    props.notFound === true ? m.error_page_not_found_body() : m.error_page_unexpected_body();
  const message = () => describeError(props.error);

  useDocumentTitle(title, { suffix: " - FlexAgent", restore: true });

  return (
    <div
      class={cn(
        "bg-background text-foreground flex items-center justify-center p-6",
        props.class ?? "min-h-full",
      )}
    >
      <div class="flex max-w-md flex-col items-center gap-3 text-center">
        <IconTriangleAlert class="text-warn size-8" aria-hidden="true" />
        <h1 class="text-lg font-semibold">{title()}</h1>
        <p class="text-muted-foreground text-sm">{body()}</p>
        <div class="mt-1 flex items-center gap-2">
          <Button onClick={() => location.reload()}>{m.error_page_reload()}</Button>
          <Button variant="outline" onClick={() => history.back()}>
            {m.error_page_go_back()}
          </Button>
        </div>
        <details class="mt-2 w-full text-left">
          <summary class="text-muted-foreground hover:text-foreground cursor-pointer text-[11px]">
            {m.error_page_details()}
          </summary>
          <div class="text-muted-foreground mt-1 flex flex-col gap-1 text-[11px]">
            <Show when={props.notFound === true}>
              <p class="font-mono">status 404</p>
            </Show>
            <Show when={message()}>
              {(value) => <p class="font-mono break-words">{value()}</p>}
            </Show>
          </div>
        </details>
      </div>
    </div>
  );
}
