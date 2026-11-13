/**
 * Stable client identity for Web Push targeting.
 *
 * `clientId` is per browser profile / installed PWA and persisted in
 * `localStorage`. The Service Worker cannot read `localStorage`, so the value is
 * mirrored into IndexedDB for `pushsubscriptionchange`.
 */

const CLIENT_ID_KEY = "fxg.client.id";
const DB_NAME = "fxg-client";
const STORE_NAME = "identity";
const CLIENT_ID_RECORD = "client_id";

let cached: string | null = null;
let persistenceStarted = false;

function openIdentityDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.addEventListener("upgradeneeded", () => {
      request.result.createObjectStore(STORE_NAME);
    });
    request.addEventListener("success", () => resolve(request.result));
    request.addEventListener("error", () =>
      reject(request.error ?? new Error("indexedDB open failed")),
    );
  });
}

/** Mirrors the client ID into IndexedDB so the Service Worker can re-register. */
async function persistClientId(id: string): Promise<void> {
  try {
    const db = await openIdentityDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readwrite");
      tx.objectStore(STORE_NAME).put(id, CLIENT_ID_RECORD);
      tx.addEventListener("complete", () => resolve());
      tx.addEventListener("error", () => reject(tx.error ?? new Error("indexedDB write failed")));
      tx.addEventListener("abort", () => reject(tx.error ?? new Error("indexedDB write aborted")));
    });
    db.close();
  } catch {
    // Best effort; the page re-registers the subscription on the next start.
  }
}

/** Reads the client ID mirrored by the page (Service Worker). */
export async function readClientIdFromIndexedDb(): Promise<string | null> {
  try {
    const db = await openIdentityDb();
    const value = await new Promise<string | null>((resolve, reject) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const request = tx.objectStore(STORE_NAME).get(CLIENT_ID_RECORD);
      request.addEventListener("success", () => {
        resolve(typeof request.result === "string" ? request.result : null);
      });
      request.addEventListener("error", () =>
        reject(request.error ?? new Error("indexedDB read failed")),
      );
    });
    db.close();
    return value;
  } catch {
    return null;
  }
}

/** Returns the stable ID of this browser profile / installed PWA. */
export function clientId(): string {
  if (cached !== null) return cached;

  let id: string | null = null;
  try {
    id = localStorage.getItem(CLIENT_ID_KEY);
  } catch {
    // localStorage can be unavailable (private mode / sandboxed iframe).
  }
  if (id === null || id === "") {
    id = crypto.randomUUID();
    try {
      localStorage.setItem(CLIENT_ID_KEY, id);
    } catch {
      // Keep the in-memory value for this page load.
    }
  }
  cached = id;
  if (!persistenceStarted) {
    persistenceStarted = true;
    void persistClientId(id);
  }
  return id;
}

/** Generates an ID for one Client WS connection (tab). */
export function newTabId(): string {
  return crypto.randomUUID();
}
