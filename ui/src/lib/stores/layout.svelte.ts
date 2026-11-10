/**
 * Reactive layout state store for mobile drawer and desktop sidebar collapse.
 */
class LayoutStore {
  mobileDrawerOpen = $state(false);
  sidebarCollapsed = $state(false);

  constructor() {
    if (typeof localStorage !== "undefined") {
      this.sidebarCollapsed = localStorage.getItem("fxg:sidebar_collapsed") === "true";
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
}

export const layoutState = new LayoutStore();
