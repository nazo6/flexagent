/**
 * Reactive state helpers for the module-level stores.
 *
 * Solid 2 signal reads return the value committed by the last flush, while the
 * Svelte stores being ported are written and read synchronously: an event
 * handler, a REST response or a WebSocket message writes state, and the same
 * turn reads it back (and the ported unit tests assert on it immediately). A
 * cell therefore keeps the value in a plain field and uses a signal only as a
 * change notification:
 *
 * - `get()` returns the current value immediately and subscribes the caller.
 * - `set()` / `update()` write immediately and notify subscribers, which read
 *   the new value on the next flush.
 *
 * `ownedWrite` is enabled because these stores are mutated from lifecycle hooks
 * and effects (`onSettled`, `createEffect`) as well as event handlers; without
 * it Solid 2 throws `REACTIVE_WRITE_IN_OWNED_SCOPE`.
 */
import { createSignal, type Accessor, type Setter } from "solid-js";

/** Change notification shared by the cells and collections below. */
class Notifier {
  readonly #read: Accessor<number>;
  readonly #notify: Setter<number>;

  constructor() {
    [this.#read, this.#notify] = createSignal(0, { ownedWrite: true });
  }

  /** Subscribes the current reader (no-op outside a tracking scope). */
  track(): void {
    this.#read();
  }

  notify(): void {
    this.#notify((version) => version + 1);
  }
}

/** A mutable value that notifies Solid readers when it changes. */
export class ReactiveCell<T> {
  #value: T;
  readonly #notifier = new Notifier();

  constructor(initial: T) {
    this.#value = initial;
  }

  /** Reads the current value and subscribes the caller to changes. */
  get(): T {
    this.#notifier.track();
    return this.#value;
  }

  set(next: T): void {
    if (Object.is(next, this.#value)) return;
    this.#value = next;
    this.#notifier.notify();
  }

  /** Applies `change` to the current value, stores and returns the result. */
  update(change: (current: T) => T): T {
    const next = change(this.#value);
    this.set(next);
    return next;
  }

  /** Notifies readers after an in-place mutation of the held value. */
  touch(): void {
    this.#notifier.notify();
  }
}

/**
 * A `Map` that notifies Solid readers.
 *
 * The backing map stays a plain `Map` (reads are immediate) and the ported
 * reducer mutates the buffers in place through the standard `Map` API.
 */
export class ReactiveMap<K, V> implements Map<K, V> {
  readonly #map = new Map<K, V>();
  readonly #notifier = new Notifier();

  get size(): number {
    this.#notifier.track();
    return this.#map.size;
  }

  get [Symbol.toStringTag](): string {
    return "Map";
  }

  get(key: K): V | undefined {
    this.#notifier.track();
    return this.#map.get(key);
  }

  has(key: K): boolean {
    this.#notifier.track();
    return this.#map.has(key);
  }

  set(key: K, value: V): this {
    this.#map.set(key, value);
    this.#notifier.notify();
    return this;
  }

  delete(key: K): boolean {
    const deleted = this.#map.delete(key);
    if (deleted) this.#notifier.notify();
    return deleted;
  }

  clear(): void {
    if (this.#map.size === 0) return;
    this.#map.clear();
    this.#notifier.notify();
  }

  forEach(callback: (value: V, key: K, map: Map<K, V>) => void, thisArg?: unknown): void {
    this.#notifier.track();
    for (const [key, value] of this.#map) callback.call(thisArg, value, key, this);
  }

  entries(): IterableIterator<[K, V]> {
    this.#notifier.track();
    return this.#map.entries();
  }

  keys(): IterableIterator<K> {
    this.#notifier.track();
    return this.#map.keys();
  }

  values(): IterableIterator<V> {
    this.#notifier.track();
    return this.#map.values();
  }

  [Symbol.iterator](): IterableIterator<[K, V]> {
    return this.entries();
  }
}

/** A `Set` that notifies Solid readers (same rationale as {@link ReactiveMap}). */
export class ReactiveSet<T> implements Set<T> {
  readonly #set = new Set<T>();
  readonly #notifier = new Notifier();

  get size(): number {
    this.#notifier.track();
    return this.#set.size;
  }

  get [Symbol.toStringTag](): string {
    return "Set";
  }

  has(value: T): boolean {
    this.#notifier.track();
    return this.#set.has(value);
  }

  add(value: T): this {
    if (this.#set.has(value)) return this;
    this.#set.add(value);
    this.#notifier.notify();
    return this;
  }

  delete(value: T): boolean {
    const deleted = this.#set.delete(value);
    if (deleted) this.#notifier.notify();
    return deleted;
  }

  clear(): void {
    if (this.#set.size === 0) return;
    this.#set.clear();
    this.#notifier.notify();
  }

  forEach(callback: (value: T, value2: T, set: Set<T>) => void, thisArg?: unknown): void {
    this.#notifier.track();
    for (const value of this.#set) callback.call(thisArg, value, value, this);
  }

  entries(): IterableIterator<[T, T]> {
    this.#notifier.track();
    return this.#set.entries();
  }

  keys(): IterableIterator<T> {
    this.#notifier.track();
    return this.#set.keys();
  }

  values(): IterableIterator<T> {
    this.#notifier.track();
    return this.#set.values();
  }

  [Symbol.iterator](): IterableIterator<T> {
    return this.values();
  }
}
