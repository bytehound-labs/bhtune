import type {
  OpcBrowseResponse,
  OpcReadResponse,
  OpcTagNodeResponse,
} from "../../api/opc";

export type QualityWarning = {
  selectedTag: string;
  reading: OpcReadResponse;
};

export type SelectedNode = {
  nodeKey: string;
  itemId: string;
};

export type ScopeState = {
  status: "loading" | "loading-more" | "loaded" | "error";
  nodes: OpcTagNodeResponse[];
  nextPageToken: string | null;
  complete: boolean;
  warning: string | null;
  message?: string;
};

export type ScopeSnapshot = Omit<ScopeState, "status" | "message">;

export const ROOT_SCOPE_KEY = "__root__";

export const BROWSE_PAGE_SIZE = 200;

export const MAX_ROOT_SCOPE_PAGES = 32;

export function scopeKey(parentNodeKey: string | null): string {
  return parentNodeKey ?? ROOT_SCOPE_KEY;
}

export function nodeCanExpand(node: OpcTagNodeResponse): boolean {
  return node.kind === "branch" || node.kind === "branch_and_item";
}

export function nodeCanSelect(node: OpcTagNodeResponse): boolean {
  return Boolean(nodeItemId(node));
}

export function nodeItemId(node: OpcTagNodeResponse): string | null {
  return node.item_id || null;
}

export function nodeKindLabel(node: OpcTagNodeResponse): string | null {
  if (node.kind === "branch_and_item") return "branch + tag";
  if (nodeCanSelect(node)) return "tag";
  return null;
}

export function mergePage(
  previous: ScopeState | undefined,
  page: OpcBrowseResponse,
  append: boolean,
): ScopeSnapshot {
  return {
    nodes: append ? [...(previous?.nodes ?? []), ...page.nodes] : page.nodes,
    nextPageToken: page.next_page_token ?? null,
    complete: page.complete,
    warning: page.warning ?? null,
  };
}
