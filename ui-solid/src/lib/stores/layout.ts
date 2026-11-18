import { createRoot } from "solid-js";
import { ReactiveCell } from "#lib/stores/reactive.ts";

/**
 * Reactive layout state store for mobile drawer and desktop sidebar collapse.
 */
export class LayoutStore {
  readonly #mobileDrawerOpen = new ReactiveCell(false);
  readonly #sidebarCollapsed: ReactiveCell<boolean>;
  readonly #rightRailCollapsed: ReactiveCell<boolean>;

  constructor() {
    let sidebarCollapsed = false;
    let rightRailCollapsed = false;
    if (typeof localStorage !== "undefined") {
      try {
        sidebarCollapsed = localStorage.getItem("fxg:sidebar_collapsed") === "true";
        rightRailCollapsed = localStorage.getItem("fxg:right_rail_collapsed") === "true";
      } catch {
        // Storage may be disabled; keep the defaults.
      }
    }
    this.#sidebarCollapsed = new ReactiveCell(sidebarCollapsed);
    this.#rightRailCollapsed = new ReactiveCell(rightRailCollapsed);
  }

  get mobileDrawerOpen(): boolean {
    return this.#mobileDrawerOpen.get();
  }

  set mobileDrawerOpen(open: boolean) {
    this.#mobileDrawerOpen.set(open);
  }

  get sidebarCollapsed(): boolean {
    return this.#sidebarCollapsed.get();
  }

  set sidebarCollapsed(collapsed: boolean) {
    this.#sidebarCollapsed.set(collapsed);
  }

  get rightRailCollapsed(): boolean {
    return this.#rightRailCollapsed.get();
  }

  set rightRailCollapsed(collapsed: boolean) {
    this.#rightRailCollapsed.set(collapsed);
  }

  toggleMobileDrawer(): void {
    this.mobileDrawerOpen = !this.mobileDrawerOpen;
  }

  setMobileDrawer(open: boolean): void {
    this.mobileDrawerOpen = open;
  }

  toggleSidebar(): void {
    this.setSidebarCollapsed(!this.sidebarCollapsed);
  }

  setSidebarCollapsed(collapsed: boolean): void {
    this.sidebarCollapsed = collapsed;
    if (typeof localStorage !== "undefined") {
      try {
        localStorage.setItem("fxg:sidebar_collapsed", String(collapsed));
      } catch {
        // Ignore quota/storage errors
      }
    }
  }

  toggleRightRail(): void {
    this.setRightRailCollapsed(!this.rightRailCollapsed);
  }

  setRightRailCollapsed(collapsed: boolean): void {
    this.rightRailCollapsed = collapsed;
    if (typeof localStorage !== "undefined") {
      try {
        localStorage.setItem("fxg:right_rail_collapsed", String(collapsed));
      } catch {
        // Ignore quota/storage errors
      }
    }
  }
}

export const layoutState = createRoot(() => new LayoutStore());
