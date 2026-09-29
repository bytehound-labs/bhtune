import { describe, expect, it, vi } from "vitest";
import type {
  OpcBrowseResponse,
  OpcIndexedSearchMatchResponse,
  OpcLiveSearchMatch,
  OpcSearchIndexStatusResponse,
  OpcTagNodeResponse,
} from "../../api/opc";
import type { ScopeState } from "./browseModel";
import {
  mergePage,
  nodeCanExpand,
  nodeCanSelect,
  nodeItemId,
  nodeKindLabel,
  scopeKey,
} from "./browseModel";
import {
  highlightedRanges,
  hasUsableIndex,
  nextSearchIndex,
  searchMatchMode,
} from "./searchModel";
import {
  fallbackSelectedNode,
  reduceRestorePhase,
  restoreLocateSettled,
  restoreOverlayPending,
  revealMatchFromIndexedSearch,
  revealMatchFromLiveSearch,
  rootScopeCandidates,
  savedTagInitializationMessage,
  scrollSettleDecision,
} from "./restoreModel";
import {
  handleTreeNodeDoubleClick,
  scrollSelectedNodeIntoView,
} from "./treeModel";

function node(
  overrides: Partial<OpcTagNodeResponse> &
    Pick<OpcTagNodeResponse, "display_name" | "kind" | "node_key">,
): OpcTagNodeResponse {
  return { item_id: null, ...overrides };
}

function status(
  state: string,
  activeGeneration = 1,
): OpcSearchIndexStatusResponse {
  return {
    active_generation: activeGeneration,
    auto_refresh_enabled: false,
    database_bytes: 0,
    entry_count: 0,
    organization: "test",
    scheduler: { circuit_open: false, consecutive_failures: 0 },
    server: "server",
    source: "test",
    state,
    unique_item_count: 0,
  };
}

function page(
  nodes: OpcTagNodeResponse[],
  token: string | null = "opaque-token",
): OpcBrowseResponse {
  return {
    complete: token === null,
    nodes,
    organization: "test",
    session_id: "session-1",
    source: "live",
    next_page_token: token,
    warning: null,
  };
}

describe("OPC tag browser browse helpers", () => {
  it("keeps the root scope key and opaque page tokens intact", () => {
    expect(scopeKey(null)).toBe("__root__");
    expect(scopeKey("node-key")).toBe("node-key");
    const first = node({
      display_name: "PV",
      kind: "item",
      node_key: "a",
      item_id: "FCS0201!204FI00510.PV",
    });
    const second = node({
      display_name: "MODE",
      kind: "item",
      node_key: "b",
      item_id: "FCS0201!204FI00510.MODE",
    });
    const replaced = mergePage(undefined, page([first], "page-2"), false);
    expect(replaced.nodes).toEqual([first]);
    expect(replaced.nextPageToken).toBe("page-2");
    const previous: ScopeState = { ...replaced, status: "loaded" };
    const appended = mergePage(previous, page([second], null), true);
    expect(appended.nodes).toEqual([first, second]);
    expect(appended.nextPageToken).toBeNull();
    expect(appended.complete).toBe(true);
  });

  it("treats an empty ItemID as unselectable without splitting punctuation", () => {
    const empty = node({
      display_name: "branch",
      kind: "branch",
      node_key: "branch",
      item_id: "",
    });
    const both = node({
      display_name: "both",
      kind: "branch_and_item",
      node_key: "both",
      item_id: "FCS0201!PV",
    });
    expect(nodeItemId(empty)).toBeNull();
    expect(nodeCanSelect(empty)).toBe(false);
    expect(nodeCanExpand(both)).toBe(true);
    expect(nodeCanSelect(both)).toBe(true);
    expect(nodeKindLabel(both)).toBe("branch + tag");
    expect(nodeKindLabel(empty)).toBeNull();
  });
});

describe("OPC tag browser search helpers", () => {
  it("enables indexed search only for a usable generation", () => {
    expect(hasUsableIndex(undefined)).toBe(false);
    expect(hasUsableIndex(status("ready", 0))).toBe(false);
    expect(hasUsableIndex(status("partial"))).toBe(false);
    expect(hasUsableIndex(status("not_indexed"))).toBe(false);
    expect(hasUsableIndex(status("deleting"))).toBe(false);
    expect(hasUsableIndex(status("ready"))).toBe(true);
    expect(hasUsableIndex(status("stale"))).toBe(true);
    expect(hasUsableIndex(status("refreshing"))).toBe(true);
    expect(hasUsableIndex(status("failed"))).toBe(true);
  });

  it("highlights case-insensitive terms without treating punctuation as a boundary", () => {
    expect(highlightedRanges("FCS0201!PV", "pv")).toEqual([[8, 10]]);
    expect(highlightedRanges("FCS0201!PV", "")).toEqual([]);
    expect(highlightedRanges("FCS0201!PV", "fcs pv")).toEqual([
      [0, 3],
      [8, 10],
    ]);
    expect(searchMatchMode("ab")).toBe("prefix");
    expect(searchMatchMode("abc")).toBe("contains");
  });

  it("wraps keyboard search navigation", () => {
    expect(nextSearchIndex(-1, 1, 3)).toBe(1);
    expect(nextSearchIndex(-1, -1, 3)).toBe(1);
    expect(nextSearchIndex(0, -1, 3)).toBe(2);
    expect(nextSearchIndex(2, 1, 3)).toBe(0);
  });
});

describe("OPC tag browser restoration helpers", () => {
  const target = "FCS0201!204FI00510.PV";

  it("maps indexed and live reveal matches without inventing node keys", () => {
    const indexed: OpcIndexedSearchMatchResponse = {
      breadcrumbs: ["FCS0201", "204FI00510"],
      display_name: "PV",
      item_id: target,
      kind: "item",
    };
    expect(revealMatchFromIndexedSearch(indexed)).toEqual({
      itemId: target,
      displayName: "PV",
      nodeKey: null,
      breadcrumbs: [
        { displayName: "FCS0201", nodeKey: null },
        { displayName: "204FI00510", nodeKey: null },
      ],
    });

    const live: OpcLiveSearchMatch = {
      node: {
        node_key: "pv",
        display_name: "PV",
        kind: "item",
        item_id: "",
      },
      breadcrumbs: [],
    };
    expect(revealMatchFromLiveSearch(live)).toBeNull();

    const scopeRoot = node({
      display_name: "FCS0201",
      kind: "branch",
      node_key: "root",
      item_id: "FCS0201",
    });
    const revealed = revealMatchFromLiveSearch(
      {
        node: {
          node_key: "pv",
          display_name: "PV",
          kind: "item",
          item_id: target,
        },
        breadcrumbs: [
          { node_key: "root", display_name: "FCS0201" },
          { node_key: "loop", display_name: "204FI00510" },
        ],
      },
      scopeRoot,
    );
    expect(revealed?.breadcrumbs).toEqual([
      { displayName: "FCS0201", nodeKey: "root" },
      { displayName: "204FI00510", nodeKey: "loop" },
    ]);
  });

  it("ranks expandable ItemID prefixes without splitting separators", () => {
    const candidates = rootScopeCandidates(
      [
        node({
          display_name: "other",
          kind: "branch",
          node_key: "other",
          item_id: "OTHER",
        }),
        node({
          display_name: "item",
          kind: "item",
          node_key: "item",
          item_id: "FCS0201",
        }),
        node({
          display_name: "controller",
          kind: "branch",
          node_key: "controller",
          item_id: "FCS0201",
        }),
        node({
          display_name: "loop",
          kind: "branch_and_item",
          node_key: "loop",
          item_id: "FCS0201!204FI00510",
        }),
      ],
      target,
    );
    expect(candidates.map((candidate) => candidate.node_key)).toEqual([
      "loop",
      "controller",
    ]);
  });

  it("advances the saved-tag overlay only through its allowed transitions", () => {
    expect(reduceRestorePhase("idle", "start")).toBe("locating");
    expect(reduceRestorePhase("locating", "located")).toBe("awaiting-scroll");
    expect(reduceRestorePhase("awaiting-scroll", "clear-overlay")).toBe(
      "settled",
    );
    expect(reduceRestorePhase("settled", "located")).toBe("settled");
    expect(reduceRestorePhase("idle", "located")).toBe("idle");
    expect(reduceRestorePhase("locating", "clear-overlay")).toBe("locating");
    expect(restoreOverlayPending("locating")).toBe(true);
    expect(restoreOverlayPending("settled")).toBe(false);
    expect(restoreLocateSettled("awaiting-scroll")).toBe(true);
    expect(restoreLocateSettled("locating")).toBe(false);
    expect(savedTagInitializationMessage("  tag  ")).toBe(
      "Locating saved tag…",
    );
    expect(savedTagInitializationMessage("   ")).toBe("Loading tags…");
    expect(
      fallbackSelectedNode([
        node({ display_name: "branch", kind: "branch", node_key: "b" }),
        node({
          display_name: "PV",
          kind: "item",
          node_key: "pv",
          item_id: target,
        }),
      ]),
    ).toEqual({ nodeKey: "pv", itemId: target });
  });

  it("retries scroll settlement for a bounded number of frames", () => {
    expect(scrollSettleDecision(true, 1)).toBe("clear");
    expect(scrollSettleDecision(false, 59)).toBe("retry");
    expect(scrollSettleDecision(false, 60)).toBe("stop");
  });
});

describe("OPC tag browser tree helpers", () => {
  it("double-clicks expandable and selectable nodes differently", () => {
    const branch = node({
      display_name: "branch",
      kind: "branch",
      node_key: "branch",
    });
    const tag = node({
      display_name: "PV",
      kind: "item",
      node_key: "pv",
      item_id: "FCS0201!PV",
    });
    const plain = node({
      display_name: "plain",
      kind: "unspecified",
      node_key: "plain",
    });
    const toggle = vi.fn();
    const confirm = vi.fn();
    handleTreeNodeDoubleClick(branch, toggle, confirm);
    handleTreeNodeDoubleClick(tag, toggle, confirm);
    handleTreeNodeDoubleClick(plain, toggle, confirm);
    expect(toggle.mock.calls.map(([called]) => called.node_key)).toEqual([
      "branch",
      "plain",
    ]);
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(confirm).toHaveBeenCalledWith(tag);
  });

  it("scrolls a selected row into the inset viewport", () => {
    let scrollTop = 0;
    const viewport = {
      clientTop: 0,
      clientHeight: 100,
      get scrollTop() {
        return scrollTop;
      },
      set scrollTop(value: number) {
        scrollTop = value;
      },
      getBoundingClientRect: () => ({ top: 0, bottom: 100 }),
    };
    const selected = {
      getBoundingClientRect: () => ({
        top: 0 - scrollTop,
        bottom: 20 - scrollTop,
      }),
    };
    expect(
      scrollSelectedNodeIntoView(
        viewport as unknown as HTMLDivElement,
        selected as unknown as HTMLButtonElement,
      ),
    ).toBe(true);
    expect(scrollTop).toBe(-4);
  });
});
