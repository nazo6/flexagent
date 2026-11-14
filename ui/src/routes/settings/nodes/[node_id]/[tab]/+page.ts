import { error } from "@sveltejs/kit";
import { isNodeTab } from "#lib/settings-routes.ts";
import type { PageLoad } from "./$types";

export const load: PageLoad = ({ params }) => {
  if (!isNodeTab(params.tab)) {
    error(404, "Not found");
  }
  return { nodeId: params.node_id, tab: params.tab };
};
