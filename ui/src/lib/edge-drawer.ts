/** Finger travel from the left edge that starts the pull-out gesture. */
export const EDGE_PX = 24;

/** Movement before we decide between a vertical scroll and a drawer drag. */
export const DIRECTION_SLOP_PX = 6;

/** Fraction of the drawer width that settles open / closed. */
export const OPEN_THRESHOLD = 0.35;
export const CLOSE_THRESHOLD = 0.65;

/** Fling speed in px/ms that settles the drawer even before the threshold. */
export const FLING_VELOCITY = 0.45;

/** A fling must have travelled this far; a tiny quick move is not a fling. */
export const FLING_MIN_DISTANCE_PX = 32;

export interface DragSample {
  /** 0 = fully closed, 1 = fully open. */
  progress: number;
  /** Travel in the settling direction (positive for both open and close). */
  distance: number;
  /** px/ms; positive opens, negative closes. */
  velocity: number;
}

/** Whether an edge drag that is being released should settle open. */
export function shouldSettleOpen(sample: DragSample): boolean {
  return (
    sample.progress >= OPEN_THRESHOLD ||
    (sample.distance >= FLING_MIN_DISTANCE_PX &&
      sample.velocity > FLING_VELOCITY &&
      sample.progress > 0.05)
  );
}

/** Whether a drag on the open panel should settle closed. */
export function shouldSettleClosed(sample: DragSample): boolean {
  return (
    sample.progress <= CLOSE_THRESHOLD ||
    (sample.distance >= FLING_MIN_DISTANCE_PX &&
      sample.velocity < -FLING_VELOCITY &&
      sample.progress < 0.95)
  );
}
