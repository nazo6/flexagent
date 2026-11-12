/**
 * Reactive layout state store for mobile drawer and desktop sidebar collapse.
 */
class LayoutStore {
  mobileDrawerOpen = $state(false);
  sidebarCollapsed = $state(false);
  rightRailCollapsed = $state(false);

  constructor() {
    if (typeof localStorage !== "undefined") {
      this.sidebarCollapsed = localStorage.getItem("fxg:sidebar_collapsed") === "true";
      this.rightRailCollapsed = localStorage.getItem("fxg:right_rail_collapsed") === "true";
    }
  }

  toggleMobileDrawer(): void {
    this.mobileDrawerOpen = !this.mobileDrawerOpen;
  }

  setMobileDrawer(open: boolean): void {
    this.mobileDrawerOpen = open;
  }

  toggleSidebar(): void {
    this.sidebarCollapsed = !this.sidebarCollapsed;
    if (typeof localStorage !== "undefined") {
      try {
        localStorage.setItem("fxg:sidebar_collapsed", String(this.sidebarCollapsed));
      } catch {
        // Ignore quota/storage errors
      }
    }
  }

  setSidebarCollapsed(collapsed: boolean): void {
    this.sidebarCollapsed = collapsed;
    if (typeof localStorage !== "undefined") {
      try {
        localStorage.setItem("fxg:sidebar_collapsed", String(this.sidebarCollapsed));
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
        localStorage.setItem("fxg:right_rail_collapsed", String(this.rightRailCollapsed));
      } catch {
        // Ignore quota/storage errors
      }
    }
  }
}

export const layoutState = new LayoutStore();
