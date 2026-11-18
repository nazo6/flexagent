import { createRouter, defineRoutes } from "@solidjs/router";
import AppShell from "./shells/AppShell";
import Home from "../routes/Home";
import NotFound from "../routes/NotFound";
import Styleguide from "../routes/Styleguide";

const routes = defineRoutes([
  {
    path: "/",
    component: AppShell,
    children: [
      { path: "/", component: Home },
      { path: "/styleguide", component: Styleguide },
      { path: "*404", component: NotFound },
    ],
  },
]);

export const Router = createRouter({ routes });
export const { paths } = Router;
