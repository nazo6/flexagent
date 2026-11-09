/**
 * Minimal declarative data fetching for Svelte 5 runes.
 *
 * The store keeps the `data` / `loading` / `error` state and exposes `refresh`
 * (and `invalidate` for keyed resources). Components stay declarative; they do
 * not hand-roll loading flags and error handling.
 *
 * This intentionally stays small: add TanStack Query only if caching, retries,
 * request deduplication or optimistic updates are actually needed.
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
  let data = $state<T | undefined>(undefined);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let generation = 0;

  async function refresh(loadOptions: ResourceLoadOptions = {}): Promise<void> {
    if (loading) return;
    const current = ++generation;
    loading = true;
    error = null;
    try {
      const next = await load();
      if (generation !== current) return;
      data = next;
    } catch (caught) {
      if (generation !== current) return;
      error = errorMessage(caught);
      if (!loadOptions.silent) options.onError?.(caught);
    } finally {
      if (generation === current) loading = false;
    }
  }

  function reset(): void {
    generation += 1;
    data = undefined;
    loading = false;
    error = null;
  }

  return {
    get data() {
      return data;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
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
  let entries = $state<Record<string, ResourceEntry<T>>>({});
  const generations = new Map<K, number>();

  function entry(key: K): ResourceEntry<T> {
    return entries[key] ?? IDLE;
  }

  async function loadKey(key: K, loadOptions: ResourceLoadOptions = {}): Promise<void> {
    if (entry(key).loading) return;
    const current = (generations.get(key) ?? 0) + 1;
    generations.set(key, current);
    entries[key] = { data: entry(key).data, loading: true, error: null };
    try {
      const next = await load(key);
      if (generations.get(key) !== current) return;
      entries[key] = { data: next, loading: false, error: null };
    } catch (caught) {
      if (generations.get(key) !== current) return;
      entries[key] = { data: entry(key).data, loading: false, error: errorMessage(caught) };
      if (!loadOptions.silent) options.onError?.(caught, key);
    }
  }

  function invalidate(key: K): void {
    generations.set(key, (generations.get(key) ?? 0) + 1);
    entries[key] = IDLE;
  }

  return { entry, load: loadKey, invalidate };
}
