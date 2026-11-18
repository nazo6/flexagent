import type { JSX } from "@solidjs/web";

/** Settings shell. Full navigation lands with the settings port. */
export default function SettingsShell(props: { children?: JSX.Element }) {
  return (
    <div class="desk-page">
      <p class="desk-label">Settings</p>
      <div class="mt-2">{props.children}</div>
    </div>
  );
}
