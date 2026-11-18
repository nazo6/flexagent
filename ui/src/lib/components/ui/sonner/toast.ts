import { ToasterStore, useToaster } from "terracotta/toast";

/**
 * Toast queue, replacing `svelte-sonner`.
 *
 * The queue lives at module scope so any module can push a toast — that is how
 * the Svelte app used `toast(...)` from event handlers and stores. `Toaster`
 * only subscribes to it.
 *
 * `toast.promise` is not implemented: nothing in `ui/src` used it. Success,
 * error, info, warning, loading, default and `dismiss` cover every call site.
 */

export type ToastKind = "default" | "success" | "error" | "info" | "warning" | "loading";

export interface ToastItem {
  kind: ToastKind;
  message: string;
  description: string | undefined;
}

export interface ToastOptions {
  description?: string;
  /** Milliseconds before the toast disappears; `0` keeps it until dismissed. */
  duration?: number;
}

const DEFAULT_DURATION = 4000;

const store = new ToasterStore<ToastItem>();

/** The live queue, for the `Toaster` component. */
export function useToastQueue() {
  return useToaster(store);
}

function push(kind: ToastKind, message: string, options?: ToastOptions): string {
  const duration = options?.duration ?? (kind === "loading" ? 0 : DEFAULT_DURATION);
  const id = store.create({ kind, message, description: options?.description });
  if (duration > 0) setTimeout(() => store.remove(id), duration);
  return id;
}

/** Dismisses one toast by id, or every toast when called without one. */
function dismiss(id?: string): void {
  if (id === undefined) store.clear();
  else store.remove(id);
}

type ToastFunction = {
  (message: string, options?: ToastOptions): string;
  success(message: string, options?: ToastOptions): string;
  error(message: string, options?: ToastOptions): string;
  info(message: string, options?: ToastOptions): string;
  warning(message: string, options?: ToastOptions): string;
  loading(message: string, options?: ToastOptions): string;
  dismiss(id?: string): void;
};

/**
 * Shows a toast. `toast("saved")`, `toast.success("saved")`, ...
 */
export const toast: ToastFunction = Object.assign(
  (message: string, options?: ToastOptions) => push("default", message, options),
  {
    success: (message: string, options?: ToastOptions) => push("success", message, options),
    error: (message: string, options?: ToastOptions) => push("error", message, options),
    info: (message: string, options?: ToastOptions) => push("info", message, options),
    warning: (message: string, options?: ToastOptions) => push("warning", message, options),
    loading: (message: string, options?: ToastOptions) => push("loading", message, options),
    dismiss,
  },
);
