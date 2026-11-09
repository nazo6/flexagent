import { describe, expect, it, vi } from "vitest";
import { createKeyedResource, createResource } from "./resource.svelte.ts";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("createResource", () => {
  it("loads data and toggles loading", async () => {
    const load = vi.fn<() => Promise<string>>().mockResolvedValue("value");
    const resource = createResource(load);

    expect(resource.data).toBeUndefined();
    const pending = resource.refresh();
    expect(resource.loading).toBe(true);

    await pending;
    expect(resource.data).toBe("value");
    expect(resource.loading).toBe(false);
    expect(resource.error).toBeNull();
  });

  it("keeps the previous value and reports errors", async () => {
    const failure = new Error("boom");
    const onError = vi.fn<(error: unknown) => void>();
    let fail = false;
    const resource = createResource(
      async () => {
        if (fail) throw failure;
        return "ok";
      },
      { onError },
    );

    await resource.refresh();
    fail = true;
    await resource.refresh();

    expect(resource.data).toBe("ok");
    expect(resource.error).toBe("boom");
    expect(onError).toHaveBeenCalledWith(failure);
  });

  it("ignores concurrent refreshes", async () => {
    const pending = deferred<string>();
    const load = vi.fn<() => Promise<string>>(() => pending.promise);
    const resource = createResource(load);

    const first = resource.refresh();
    await resource.refresh();
    expect(load).toHaveBeenCalledTimes(1);

    pending.resolve("done");
    await first;
    expect(resource.data).toBe("done");
  });

  it("drops an in-flight result after reset", async () => {
    const pending = deferred<string>();
    const resource = createResource(() => pending.promise);

    const first = resource.refresh();
    resource.reset();
    pending.resolve("stale");
    await first;

    expect(resource.data).toBeUndefined();
    expect(resource.loading).toBe(false);
  });
});

describe("createKeyedResource", () => {
  it("loads each key independently", async () => {
    const resource = createKeyedResource(async (key: string) => `${key}-data`);

    expect(resource.entry("a").data).toBeUndefined();
    await resource.load("a");

    expect(resource.entry("a").data).toBe("a-data");
    expect(resource.entry("a").loading).toBe(false);
    expect(resource.entry("b").data).toBeUndefined();
  });

  it("invalidates one key only", async () => {
    const resource = createKeyedResource(async (key: string) => key);

    await Promise.all([resource.load("a"), resource.load("b")]);
    resource.invalidate("a");

    expect(resource.entry("a").data).toBeUndefined();
    expect(resource.entry("b").data).toBe("b");
  });

  it("ignores a stale response after invalidation", async () => {
    const pending = deferred<number>();
    const resource = createKeyedResource(() => pending.promise);

    const first = resource.load("a");
    resource.invalidate("a");
    pending.resolve(1);
    await first;

    expect(resource.entry("a").data).toBeUndefined();
  });

  it("reports errors with the key and keeps the previous value", async () => {
    const failure = new Error("offline");
    const onError = vi.fn<(error: unknown, key: string) => void>();
    let fail = false;
    const resource = createKeyedResource(
      async () => {
        if (fail) throw failure;
        return 1;
      },
      { onError },
    );

    await resource.load("a");
    fail = true;
    await resource.load("a");

    expect(resource.entry("a").data).toBe(1);
    expect(resource.entry("a").error).toBe("offline");
    expect(onError).toHaveBeenCalledWith(failure, "a");
  });
});
