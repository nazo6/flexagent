import { ReactiveCell } from "#lib/stores/reactive.ts";

/**
 * Minimal declarative data fetching for Solid stores.
 *
 * The store keeps the `data` / `loading` / `error` state and exposes `refresh`
 * (and `invalidate` for keyed resources). Components stay declarative; they do
 * not hand-roll loading flags and error handling.
 *
 * This intentionally stays small: add a query library only if caching, retries,
 * request deduplication or optimistic updates are actually needed.
 *
 * This is the application's own helper (not Solid's removed `createResource`
 * primitive).
 */

/** Snapshot of one resource load. */
export interface ResourceEntry<T> {
  readonly data: T | undefined;
  readonly loading: boolean;
  readonly error: string | null;
}

/** Options for a single load. */
export interface ResourceLoadOptions {
  /** Do not call `onError` (for background refreshes). */
  silent?: boolean;
}

interface ResourceOptions {
  onError?: (error: unknown) => void;
}

interface KeyedResourceOptions<K extends string> {
  onError?: (error: unknown, key: K) => void;
}

/** A resource that loads a single value. */
export interface Resource<T> {
  readonly data: T | undefined;
  readonly loading: boolean;
  readonly error: string | null;
  /** Loads (or reloads) the value; concurrent calls are ignored. */
  refresh(options?: ResourceLoadOptions): Promise<void>;
  /** Drops the current value (an in-flight load is ignored). */
  reset(): void;
}

/** A resource with one entry per string key (e.g. one worktree list per project). */
export interface KeyedResource<K extends string, T> {
  /** Current state for a key (`loading: false`, no data until `load`). */
  entry(key: K): ResourceEntry<T>;
  /** Loads (or reloads) one key; concurrent loads of the same key are ignored. */
  load(key: K, options?: ResourceLoadOptions): Promise<void>;
  /** Drops one key's value (an in-flight load is ignored). */
  invalidate(key: K): void;
}

const IDLE: ResourceEntry<never> = { data: undefined, loading: false, error: null };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Creates a resource for a single value. */
export function createResource<T>(
  load: () => Promise<T>,
  options: ResourceOptions = {},
): Resource<T> {
  const data = new ReactiveCell<T | undefined>(undefined);
  const loading = new ReactiveCell(false);
  const error = new ReactiveCell<string | null>(null);
  let generation = 0;

  async function refresh(loadOptions: ResourceLoadOptions = {}): Promise<void> {
    if (loading.get()) return;
    const current = ++generation;
    loading.set(true);
    error.set(null);
    try {
      const next = await load();
      if (generation !== current) return;
      data.set(next);
    } catch (caught) {
      if (generation !== current) return;
      error.set(errorMessage(caught));
      if (!loadOptions.silent) options.onError?.(caught);
    } finally {
      if (generation === current) loading.set(false);
    }
  }

  function reset(): void {
    generation += 1;
    data.set(undefined);
    loading.set(false);
    error.set(null);
  }

  return {
    get data() {
      return data.get();
    },
    get loading() {
      return loading.get();
    },
    get error() {
      return error.get();
    },
    refresh,
    reset,
  };
}

/** Creates a resource with one entry per key. */
export function createKeyedResource<K extends string, T>(
  load: (key: K) => Promise<T>,
  options: KeyedResourceOptions<K> = {},
): KeyedResource<K, T> {
  const entries = new ReactiveCell<Record<string, ResourceEntry<T>>>({});
  const generations = new Map<K, number>();

  function entry(key: K): ResourceEntry<T> {
    return entries.get()[key] ?? IDLE;
  }

  function update(key: K, next: ResourceEntry<T>): void {
    entries.update((current) => ({ ...current, [key]: next }));
  }

  async function loadKey(key: K, loadOptions: ResourceLoadOptions = {}): Promise<void> {
    if (entry(key).loading) return;
    const current = (generations.get(key) ?? 0) + 1;
    generations.set(key, current);
    update(key, { data: entry(key).data, loading: true, error: null });
    try {
      const next = await load(key);
      if (generations.get(key) !== current) return;
      update(key, { data: next, loading: false, error: null });
    } catch (caught) {
      if (generations.get(key) !== current) return;
      update(key, { data: entry(key).data, loading: false, error: errorMessage(caught) });
      if (!loadOptions.silent) options.onError?.(caught, key);
    }
  }

  function invalidate(key: K): void {
    generations.set(key, (generations.get(key) ?? 0) + 1);
    update(key, IDLE);
  }

  return { entry, load: loadKey, invalidate };
}
