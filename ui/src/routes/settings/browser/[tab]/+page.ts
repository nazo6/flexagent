import { error } from "@sveltejs/kit";
import { isBrowserTab } from "#lib/settings-routes.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = ({ params }) => {
  if (!isBrowserTab(params.tab)) {
    error(404, "Not found");
  }
  return { tab: params.tab };
};
