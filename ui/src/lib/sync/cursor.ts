/** Result of matching an incoming batch cursor against the current value. */
export interface CursorUpdate {
  /** Whether a source store recreation or reset (cursor rollback) was detected */
  reset: boolean;
  /** Cursor after applying the batch */
  cursor: number;
}

/**
 * Match an incoming batch cursor against the current value.
 *
 * Cursors increase monotonically in the order events are ingested from the
 * source store, so an `incoming` value smaller than `current` means the store
 * was recreated or reset (database reset). In that case report the reset and
 * adopt the new cursor; the caller discards local state such as the timeline
 * and resynchronizes from 0.
 */
export function mergeCursor(current: number | null, incoming: number): CursorUpdate {
  if (current !== null && incoming < current) {
    return { reset: true, cursor: incoming };
  }
  if (current === null || incoming > current) {
    return { reset: false, cursor: incoming };
  }
  return { reset: false, cursor: current };
}
