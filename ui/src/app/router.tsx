import { createRouter, defineRoutes } from "@solidjs/router";
import AppShell from "./shells/AppShell";
import HomePage from "../routes/HomePage";
import NotFound from "../routes/NotFound";
import ProjectsPage from "../routes/ProjectsPage";
import SearchPage from "../routes/SearchPage";
import SessionPage from "../routes/SessionPage";
import BrowserSettings from "../routes/settings/BrowserSettings";
import HubSettings from "../routes/settings/HubSettings";
import NodeSettings from "../routes/settings/NodeSettings";
import SettingsIndex from "../routes/settings/SettingsIndex";
import SettingsShell from "../routes/settings/SettingsShell";
import {
  SETTINGS_BROWSER_ROUTE,
  SETTINGS_HUB_ROUTE,
  SETTINGS_NODE_ROUTE,
  SETTINGS_NODES_ROUTE,
  SETTINGS_ROOT,
  isBrowserTab,
  isHubTab,
  isNodeTab,
} from "#lib/settings-routes.ts";

/**
 * Route table. URLs are identical to the Svelte app (deep links, PWA
 * shortcuts and history stay valid); unknown paths fall through to `*404`.
 */
const routes = defineRoutes([
  {
    path: "/",
    component: AppShell,
    children: [
      { path: "/", component: HomePage },
      { path: "/sessions/:id", component: SessionPage },
      { path: "/projects", component: ProjectsPage },
      { path: "/search", component: SearchPage },
      {
        path: SETTINGS_ROOT,
        component: SettingsShell,
        children: [
          { path: "/", component: SettingsIndex },
          { path: SETTINGS_NODES_ROUTE, component: NodeSettings },
          { path: SETTINGS_HUB_ROUTE, component: HubSettings, matchFilters: { tab: isHubTab } },
          {
            path: SETTINGS_BROWSER_ROUTE,
            component: BrowserSettings,
            matchFilters: { tab: isBrowserTab },
          },
          {
            path: SETTINGS_NODE_ROUTE,
            component: NodeSettings,
            matchFilters: { tab: isNodeTab },
          },
        ],
      },
      { path: "*404", component: NotFound },
    ],
  },
]);

export const Router = createRouter({ routes });
