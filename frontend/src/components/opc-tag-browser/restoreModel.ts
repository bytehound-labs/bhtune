import type {
  OpcIndexedSearchMatchResponse,
  OpcTagNodeResponse,
} from "../../api/opc";
import type { OpcLiveSearchMatch } from "../../api/opc";
import { nodeCanExpand, nodeCanSelect, nodeItemId } from "./browseModel";

export type RevealSearchMatch = {
  itemId: string;
  displayName: string;
  nodeKey: string | null;
  breadcrumbs: Array<{
    displayName: string;
    nodeKey: string | null;
  }>;
};

export function revealMatchFromIndexedSearch(
  match: OpcIndexedSearchMatchResponse,
): RevealSearchMatch {
  return {
    itemId: match.item_id,
    displayName: match.display_name,
    nodeKey: null,
    breadcrumbs: match.breadcrumbs.map((displayName) => ({
      displayName,
      nodeKey: null,
    })),
  };
}

export function revealMatchFromLiveSearch(
  match: OpcLiveSearchMatch,
  scopeRoot?: OpcTagNodeResponse,
): RevealSearchMatch | null {
  if (!match.node.item_id) return null;
  const breadcrumbs = match.breadcrumbs
    .filter((breadcrumb) => breadcrumb.display_name.trim().length > 0)
    .map((breadcrumb) => ({
      displayName: breadcrumb.display_name,
      nodeKey: breadcrumb.node_key,
    }));
  const normalizedBreadcrumbs = scopeRoot
    ? [
        {
          displayName: scopeRoot.display_name,
          nodeKey: scopeRoot.node_key,
        },
        ...breadcrumbs.filter(
          (breadcrumb) => breadcrumb.nodeKey !== scopeRoot.node_key,
        ),
      ]
    : breadcrumbs;
  return {
    itemId: match.node.item_id,
    displayName: match.node.display_name,
    nodeKey: match.node.node_key,
    breadcrumbs: normalizedBreadcrumbs,
  };
}

export function rootScopeCandidates(
  nodes: OpcTagNodeResponse[],
  target: string,
): OpcTagNodeResponse[] {
  return nodes
    .filter((node) => {
      const itemId = nodeItemId(node);
      return (
        nodeCanExpand(node) &&
        itemId !== null &&
        (target === itemId || target.startsWith(itemId))
      );
    })
    .sort(
      (left, right) =>
        (nodeItemId(right)?.length ?? 0) - (nodeItemId(left)?.length ?? 0),
    );
}

export function fallbackSelectedNode(
  nodes: OpcTagNodeResponse[],
): { nodeKey: string; itemId: string } | null {
  const firstSelectable = nodes.find(nodeCanSelect);
  const firstItemId = firstSelectable ? nodeItemId(firstSelectable) : null;
  return firstSelectable && firstItemId
    ? { nodeKey: firstSelectable.node_key, itemId: firstItemId }
    : null;
}

export function savedTagInitializationMessage(initialTag: string): string {
  return initialTag.trim() ? "Locating saved tag…" : "Loading tags…";
}

export type RestorePhase = "idle" | "locating" | "awaiting-scroll" | "settled";

export type RestoreEvent = "start" | "located" | "clear-overlay";

export function initialRestorePhase(hasServer: boolean): RestorePhase {
  return hasServer ? "locating" : "idle";
}

export function reduceRestorePhase(
  phase: RestorePhase,
  event: RestoreEvent,
): RestorePhase {
  if (event === "start") return "locating";
  if (event === "located") {
    return phase === "locating" ? "awaiting-scroll" : phase;
  }
  return phase === "awaiting-scroll" ? "settled" : phase;
}

export function restoreOverlayPending(phase: RestorePhase): boolean {
  return phase === "locating" || phase === "awaiting-scroll";
}

export function restoreLocateSettled(phase: RestorePhase): boolean {
  return phase === "awaiting-scroll" || phase === "settled";
}

const MAX_SCROLL_SETTLE_FRAMES = 60;

export function scrollSettleDecision(
  visible: boolean,
  attempts: number,
): "clear" | "retry" | "stop" {
  if (visible) return "clear";
  if (attempts < MAX_SCROLL_SETTLE_FRAMES) return "retry";
  return "stop";
}
