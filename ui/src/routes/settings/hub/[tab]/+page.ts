import { error } from "@sveltejs/kit";
import { isHubTab } from "#lib/settings-routes.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = ({ params }) => {
  if (!isHubTab(params.tab)) {
    error(404, "Not found");
  }
  return { tab: params.tab };
};
