import { browser } from "$app/env";

/**
 * Minimum gap between the layout and visual viewports that counts as an
 * on-screen keyboard. Smaller differences come from browser chrome or a
 * zoomed page and must not move the shell.
 */
const KEYBOARD_MIN_INSET_PX = 80;

/**
 * Keep the app shell inside the visible viewport and above an on-screen
 * keyboard.
 *
 * Chrome on Android resizes the layout viewport itself
 * (`interactive-widget=resizes-content` in app.html), so `innerHeight` already
 * excludes the keyboard. Engines that only resize the visual viewport (iOS
 * Safari) keep the layout height, so the difference is exposed as
 * `--keyboard-inset` and added to the shell's bottom safe area.
 */
export function initViewport(): () => void {
  if (!browser) return () => {};

  const root = document.documentElement;
  const viewport: VisualViewport | null = window.visualViewport ?? null;
  let frame: number | null = null;

  const update = (): void => {
    const layoutHeight = window.innerHeight;
    const visibleHeight = viewport?.height ?? layoutHeight;
    const offsetTop = viewport?.offsetTop ?? 0;
    const zoomed = viewport !== null && viewport.scale > 1.01;
    const rawInset = Math.max(0, layoutHeight - visibleHeight - offsetTop);
    const keyboardInset = zoomed || rawInset < KEYBOARD_MIN_INSET_PX ? 0 : Math.round(rawInset);

    root.style.setProperty("--app-height", `${Math.round(layoutHeight)}px`);
    root.style.setProperty("--keyboard-inset", `${keyboardInset}px`);
    if (keyboardInset > 0) {
      root.dataset.keyboard = "open";
    } else {
      delete root.dataset.keyboard;
    }
  };

  // VisualViewport fires continuously while the keyboard animates; coalesce
  // the writes into one frame.
  const schedule = (): void => {
    if (frame !== null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      update();
    });
  };

  update();
  window.addEventListener("resize", schedule);
  window.addEventListener("orientationchange", schedule);
  viewport?.addEventListener("resize", schedule);
  viewport?.addEventListener("scroll", schedule);

  return () => {
    if (frame !== null) cancelAnimationFrame(frame);
    window.removeEventListener("resize", schedule);
    window.removeEventListener("orientationchange", schedule);
    viewport?.removeEventListener("resize", schedule);
    viewport?.removeEventListener("scroll", schedule);
    root.style.removeProperty("--app-height");
    root.style.removeProperty("--keyboard-inset");
    delete root.dataset.keyboard;
  };
}
