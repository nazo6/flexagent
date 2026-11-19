/**
 * Copy text to the clipboard from any browser context.
 *
 * The async Clipboard API (`navigator.clipboard`) only exists in secure
 * contexts (HTTPS / localhost). The Web UI is commonly served over plain HTTP
 * on a LAN / Tailscale address, where `navigator.clipboard` is `undefined`.
 * Copy buttons are basic UI functionality, so fall back to the legacy
 * `document.execCommand("copy")` path (deprecated, but supported by every
 * browser for copies triggered by a user gesture).
 *
 * Throws when neither path is available or the copy is rejected; callers show
 * a localized failure toast.
 */
export async function copyText(text: string): Promise<void> {
  const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
  if (typeof clipboard?.writeText === "function") {
    try {
      await clipboard.writeText(text);
      return;
    } catch {
      // Permission denied / insecure context / document not focused: the
      // legacy path below works in more of these cases.
    }
  }

  if (!copyWithExecCommand(text)) {
    throw new Error("clipboard is unavailable");
  }
}

/**
 * Copy through a temporary off-screen textarea. `execCommand` returns a
 * boolean instead of rejecting, and restores focus to the previously active
 * element afterwards.
 */
function copyWithExecCommand(text: string): boolean {
  if (typeof document === "undefined" || document.body === null) return false;

  const previousFocus =
    document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  textarea.style.position = "fixed";
  textarea.style.top = "0";
  textarea.style.left = "-9999px";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);

  try {
    textarea.focus();
    textarea.select();
    textarea.setSelectionRange(0, text.length);
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    textarea.remove();
    previousFocus?.focus();
  }
}
