import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type RefObject,
} from "react";
import {
  useCloseOpcBrowseSession,
  useOpcBrowseFetcher,
  useOpcSearch,
  useTestOpcConnection,
} from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import type {
  OpcBrowseResponse,
  OpcReadResponse,
  OpcSearchEvent,
  OpcSearchMatchResponse,
  OpcSearchProgress,
  OpcTagNodeResponse,
} from "../api/opc";
import type { components } from "../api/schema";
import { SAMPLE_QUALITY_LABELS, SAMPLE_QUALITY_TONE } from "../lib/enumLabels";
import { deriveTag } from "../lib/opcTags";
import { Badge, Button, ErrorBanner, Modal } from "./ui";

type TemplateResponse = components["schemas"]["TemplateResponse"];
type QualityWarning = {
  selectedTag: string;
  reading: OpcReadResponse;
};

type SelectedNode = {
  nodeKey: string;
  itemId: string;
};

type ScopeState = {
  status: "loading" | "loading-more" | "loaded" | "error";
  nodes: OpcTagNodeResponse[];
  nextPageToken: string | null;
  complete: boolean;
  warning: string | null;
  message?: string;
};

type ScopeSnapshot = Omit<ScopeState, "status" | "message">;

/** Indentation step per tree depth; matches the width of the expand chevron column so a
 * leaf's label lines up under its parent branch's label, not under its chevron. */
const INDENT_PX = 18;
const ROOT_SCOPE_KEY = "__root__";
const BROWSE_PAGE_SIZE = 200;
const SEARCH_MAX_RESULTS = 25;

function scopeKey(parentNodeKey: string | null): string {
  return parentNodeKey ?? ROOT_SCOPE_KEY;
}

function nodeCanExpand(node: OpcTagNodeResponse): boolean {
  return node.kind === "branch" || node.kind === "branch_and_item";
}

function nodeCanSelect(node: OpcTagNodeResponse): boolean {
  return Boolean(nodeItemId(node));
}

function nodeItemId(node: OpcTagNodeResponse): string | null {
  return node.item_id || null;
}

function nodeKindLabel(node: OpcTagNodeResponse): string | null {
  if (node.kind === "branch_and_item") return "branch + tag";
  if (nodeCanSelect(node)) return "tag";
  return null;
}

function mergePage(
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

/** One tree level -- renders one browsed scope and recurses into whichever branch nodes are
 * expanded. Navigation uses only the gateway's opaque `node_key`; `item_id` is kept only for
 * reads/selections. */
function TreeLevel({
  parentNodeKey,
  depth,
  scopeState,
  expanded,
  onToggle,
  onSelect,
  onConfirm,
  onLoadMore,
  selectedNode,
  selectedNodeRef,
  disabled,
}: {
  parentNodeKey: string | null;
  depth: number;
  scopeState: Record<string, ScopeState>;
  expanded: Set<string>;
  onToggle: (node: OpcTagNodeResponse) => void;
  onSelect: (node: OpcTagNodeResponse) => void;
  onConfirm: (node: OpcTagNodeResponse) => void;
  onLoadMore: (parentNodeKey: string | null) => void;
  selectedNode: SelectedNode | null;
  selectedNodeRef: RefObject<HTMLButtonElement | null>;
  disabled: boolean;
}) {
  const state = scopeState[scopeKey(parentNodeKey)];
  if (!state) return null;

  if (state.status === "loading" && state.nodes.length === 0) {
    return (
      <div
        className="py-1 text-xs text-slate-500"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        Loading…
      </div>
    );
  }
  if (state.status === "error" && state.nodes.length === 0) {
    return (
      <div
        className="py-1 text-xs text-red-400"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        {state.message}
      </div>
    );
  }
  if (state.nodes.length === 0) {
    return (
      <div
        className="py-1 text-xs text-slate-500"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        No child tags.
      </div>
    );
  }

  return (
    <>
      {state.warning && (
        <div
          className="py-1 text-xs text-amber-300"
          style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
        >
          {state.warning}
        </div>
      )}
      {state.nodes.map((node) => {
        const isBranch = nodeCanExpand(node);
        const itemId = nodeItemId(node);
        const isSelected = selectedNode?.nodeKey === node.node_key;
        return (
          <div key={node.node_key}>
            <div
              className={`flex items-center gap-1.5 rounded px-1 py-1 text-sm hover:bg-slate-800 ${
                isSelected ? "bg-slate-800" : ""
              }`}
              style={{ paddingLeft: `${depth * INDENT_PX}px` }}
            >
              {isBranch ? (
                <button
                  type="button"
                  onClick={() => onToggle(node)}
                  disabled={disabled}
                  aria-label={
                    expanded.has(node.node_key) ? "Collapse" : "Expand"
                  }
                  className="w-4 shrink-0 text-slate-400 hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {expanded.has(node.node_key) ? "▾" : "▸"}
                </button>
              ) : (
                <span className="w-4 shrink-0" />
              )}
              <button
                type="button"
                onClick={() => (itemId ? onSelect(node) : onToggle(node))}
                onDoubleClick={() =>
                  itemId ? onConfirm(node) : onToggle(node)
                }
                ref={isSelected ? selectedNodeRef : undefined}
                disabled={disabled || (!itemId && !isBranch)}
                title={itemId ?? node.display_name}
                className="flex-1 truncate text-left font-mono text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {node.display_name}
              </button>
              {nodeKindLabel(node) && (
                <span className="shrink-0 text-xs text-slate-500">
                  {nodeKindLabel(node)}
                </span>
              )}
            </div>
            {isBranch && expanded.has(node.node_key) && (
              <TreeLevel
                parentNodeKey={node.node_key}
                depth={depth + 1}
                scopeState={scopeState}
                expanded={expanded}
                onToggle={onToggle}
                onSelect={onSelect}
                onConfirm={onConfirm}
                onLoadMore={onLoadMore}
                selectedNode={selectedNode}
                selectedNodeRef={selectedNodeRef}
                disabled={disabled}
              />
            )}
          </div>
        );
      })}
      {state.status === "error" && state.nodes.length > 0 && (
        <div
          className="py-1 text-xs text-red-400"
          style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
        >
          {state.message}
        </div>
      )}
      {!state.complete && state.nextPageToken && (
        <button
          type="button"
          disabled={disabled || state.status === "loading-more"}
          onClick={() => onLoadMore(parentNodeKey)}
          className="py-1 text-xs text-blue-300 hover:text-blue-200 disabled:cursor-not-allowed disabled:opacity-50"
          style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
        >
          {state.status === "loading-more" ? "Loading more…" : "Load more"}
        </button>
      )}
    </>
  );
}

/**
 * The OPC tag-tree browser modal (`ui-opc-browser`): a lazily-expanding, paged tree fed by
 * `GET /api/opc/browse`, a per-node "Read selected tag" action backed by `GET /api/opc/read`,
 * and an optional search backed by `GET /api/opc/search`. Browse navigation round-trips the
 * gateway's opaque session, node, and page tokens; display names are never parsed into paths.
 * When the user confirms a selection, the active template's process-variable suffix is
 * applied to the selected node's exact original ItemID, after a fresh quality check reads that
 * same ItemID. Reopening at a saved tag uses exact search breadcrumbs to reveal and scroll the
 * matching node when the server supports it.
 */
export function OpcTagBrowserModal({
  bridgeHost,
  opcServer,
  template,
  initialTag,
  onClose,
  onSelect,
}: {
  bridgeHost: string;
  opcServer: string;
  template: TemplateResponse | undefined;
  initialTag: string;
  onClose: () => void;
  onSelect: (tag: string) => void;
}) {
  const { fetchPage, clearCache } = useOpcBrowseFetcher(bridgeHost, opcServer);
  const closeBrowseSession = useCloseOpcBrowseSession();
  const searchOpcTags = useOpcSearch();
  const testConnection = useTestOpcConnection();
  const [scopeState, setScopeState] = useState<Record<string, ScopeState>>({});
  const scopeStateRef = useRef(scopeState);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedNode, setSelectedNode] = useState<SelectedNode | null>(null);
  const selectedNodeRef = useRef<HTMLButtonElement | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const closedSessionIdsRef = useRef<Set<string>>(new Set());
  const disposedRef = useRef(false);
  const [selectionReadError, setSelectionReadError] = useState<string | null>(
    null,
  );
  const [selectionCheckPending, setSelectionCheckPending] = useState(false);
  const [qualityWarning, setQualityWarning] = useState<QualityWarning | null>(
    null,
  );
  const [searchQuery, setSearchQuery] = useState("");
  const [searchMatches, setSearchMatches] = useState<OpcSearchMatchResponse[]>(
    [],
  );
  const [searchProgress, setSearchProgress] =
    useState<OpcSearchProgress | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const searchAbortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    scopeStateRef.current = scopeState;
  }, [scopeState]);

  function rememberSession(sessionId: string) {
    sessionIdRef.current = sessionId;
  }

  function closeSessionOnce(sessionId: string | null) {
    if (!sessionId || closedSessionIdsRef.current.has(sessionId)) {
      return;
    }
    closedSessionIdsRef.current.add(sessionId);
    closeBrowseSession.mutate({ bridgeHost, opcServer, sessionId });
  }

  function closeActiveSession() {
    const sessionId = sessionIdRef.current;
    sessionIdRef.current = null;
    closeSessionOnce(sessionId);
    clearCache();
  }

  function disposeBrowse() {
    disposedRef.current = true;
    cancelActiveSearch();
    closeActiveSession();
  }

  function cancelActiveSearch() {
    searchAbortRef.current?.abort();
  }

  async function load(
    parentNodeKey: string | null,
    options: { pageToken?: string; append?: boolean } = {},
  ): Promise<ScopeSnapshot | null> {
    const key = scopeKey(parentNodeKey);
    const previous = scopeStateRef.current[key];
    setScopeState((prev) => ({
      ...prev,
      [key]: {
        status: options.append ? "loading-more" : "loading",
        nodes: options.append ? (prev[key]?.nodes ?? []) : [],
        nextPageToken: options.append
          ? (prev[key]?.nextPageToken ?? null)
          : null,
        complete: options.append ? (prev[key]?.complete ?? false) : false,
        warning: options.append ? (prev[key]?.warning ?? null) : null,
      },
    }));
    try {
      const page = await fetchPage({
        sessionId: sessionIdRef.current ?? undefined,
        parentNodeKey: parentNodeKey ?? undefined,
        pageToken: options.pageToken,
        pageSize: BROWSE_PAGE_SIZE,
      });
      if (disposedRef.current) {
        closeSessionOnce(page.session_id);
        return null;
      }
      rememberSession(page.session_id);
      const snapshot = mergePage(previous, page, options.append ?? false);
      setScopeState((prev) => ({
        ...prev,
        [key]: { status: "loaded", ...snapshot },
      }));
      scopeStateRef.current = {
        ...scopeStateRef.current,
        [key]: { status: "loaded", ...snapshot },
      };
      return snapshot;
    } catch (err) {
      if (disposedRef.current) return null;
      const fallback = options.append ? previous : undefined;
      setScopeState((prev) => ({
        ...prev,
        [key]: {
          status: "error",
          nodes: fallback?.nodes ?? [],
          nextPageToken: fallback?.nextPageToken ?? null,
          complete: fallback?.complete ?? false,
          warning: fallback?.warning ?? null,
          message: userFacingErrorMessage(
            err,
            "Unable to load tags at this level.",
          ),
        },
      }));
      return null;
    }
  }

  async function ensureNodeVisible(
    parentNodeKey: string | null,
    nodeKey: string,
    isCancelled: () => boolean,
  ): Promise<OpcTagNodeResponse | null> {
    let state = scopeStateRef.current[scopeKey(parentNodeKey)];
    if (!state) {
      const snapshot = await load(parentNodeKey);
      if (!snapshot || isCancelled()) return null;
      state = { status: "loaded", ...snapshot };
    }

    while (!isCancelled()) {
      const match = state.nodes.find((node) => node.node_key === nodeKey);
      if (match) return match;
      if (!state.nextPageToken) return null;
      const snapshot = await load(parentNodeKey, {
        pageToken: state.nextPageToken,
        append: true,
      });
      if (!snapshot) return null;
      state = { status: "loaded", ...snapshot };
    }
    return null;
  }

  async function revealSearchMatch(
    match: OpcSearchMatchResponse,
    isCancelled: () => boolean,
  ): Promise<boolean> {
    let parentNodeKey: string | null = null;
    const breadcrumbs =
      match.breadcrumbs.at(-1)?.node_key === match.node.node_key
        ? match.breadcrumbs.slice(0, -1)
        : match.breadcrumbs;
    for (const breadcrumb of breadcrumbs) {
      const branch = await ensureNodeVisible(
        parentNodeKey,
        breadcrumb.node_key,
        isCancelled,
      );
      if (!branch || !nodeCanExpand(branch)) return false;
      setExpanded((previous) => {
        if (previous.has(branch.node_key)) return previous;
        const next = new Set(previous);
        next.add(branch.node_key);
        return next;
      });
      parentNodeKey = branch.node_key;
    }

    const node = await ensureNodeVisible(
      parentNodeKey,
      match.node.node_key,
      isCancelled,
    );
    const itemId = node ? nodeItemId(node) : nodeItemId(match.node);
    if (!itemId) return false;
    setSelectedNode({ nodeKey: match.node.node_key, itemId });
    return true;
  }

  async function revealInitialTag(
    rootNodes: OpcTagNodeResponse[],
    target: string,
    isCancelled: () => boolean,
  ): Promise<boolean> {
    const rootMatch = rootNodes.find((node) => nodeItemId(node) === target);
    if (rootMatch) {
      setSelectedNode({ nodeKey: rootMatch.node_key, itemId: target });
      return true;
    }

    const controller = new AbortController();
    searchAbortRef.current = controller;
    try {
      const result = await searchOpcTags.mutateAsync({
        bridgeHost,
        opcServer,
        query: target,
        matchMode: "exact",
        sessionId: sessionIdRef.current ?? undefined,
        maxResults: 1,
        includeBranches: false,
        signal: controller.signal,
      });
      const match = result.matches.find((candidate) => {
        return nodeItemId(candidate.node) === target;
      });
      if (!match || isCancelled() || controller.signal.aborted) return false;
      return revealSearchMatch(match, isCancelled);
    } catch {
      return false;
    } finally {
      if (searchAbortRef.current === controller) {
        searchAbortRef.current = null;
      }
    }
  }

  useEffect(() => {
    if (!opcServer) return;
    let cancelled = false;
    async function initialize() {
      const root = await load(null);
      if (cancelled || !root) return;

      const target = initialTag;
      if (
        target &&
        (await revealInitialTag(root.nodes, target, () => cancelled))
      ) {
        return;
      }

      if (!cancelled) {
        const firstSelectable = root.nodes.find(nodeCanSelect);
        const firstItemId = firstSelectable
          ? nodeItemId(firstSelectable)
          : null;
        setExpanded(new Set());
        setSelectedNode(
          firstSelectable && firstItemId
            ? {
                nodeKey: firstSelectable.node_key,
                itemId: firstItemId,
              }
            : null,
        );
      }
    }
    void initialize();
    return () => {
      cancelled = true;
      disposeBrowse();
    };
    // `load`, `revealInitialTag`, and `disposeBrowse` close over stable per-mount inputs;
    // including them would turn every render into a new browse session.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [bridgeHost, opcServer, initialTag]);

  useEffect(() => {
    selectedNodeRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedNode, scopeState, expanded]);

  function toggle(node: OpcTagNodeResponse) {
    if (!nodeCanExpand(node)) return;
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(node.node_key)) {
        next.delete(node.node_key);
      } else {
        next.add(node.node_key);
        if (!scopeStateRef.current[scopeKey(node.node_key)])
          void load(node.node_key);
      }
      return next;
    });
  }

  function selectNode(node: OpcTagNodeResponse) {
    const itemId = nodeItemId(node);
    if (!itemId) return;
    setSelectedNode({ nodeKey: node.node_key, itemId });
    setSelectionReadError(null);
    testConnection.reset();
  }

  function applyTag(tag: string) {
    const pvTag = template
      ? (deriveTag(tag, template.process_variable_suffix) ?? tag)
      : tag;
    disposeBrowse();
    onSelect(pvTag);
    onClose();
  }

  async function confirmTag(tag: string) {
    setSelectionReadError(null);
    setSelectionCheckPending(true);
    try {
      const reading = await testConnection.mutateAsync({
        bridgeHost,
        opcServer,
        tag,
      });
      if (reading.quality !== "good") {
        setQualityWarning({ selectedTag: tag, reading });
        return;
      }
      applyTag(tag);
    } catch (err) {
      setSelectionReadError(
        userFacingErrorMessage(
          err,
          "Unable to verify the selected tag's OPC quality.",
        ),
      );
    } finally {
      setSelectionCheckPending(false);
    }
  }

  function readSelectedTag() {
    if (!selectedNode) return;
    setSelectionReadError(null);
    testConnection.mutate({
      bridgeHost,
      opcServer,
      tag: selectedNode.itemId,
    });
  }

  function loadMore(parentNodeKey: string | null) {
    const state = scopeStateRef.current[scopeKey(parentNodeKey)];
    if (!state?.nextPageToken) return;
    void load(parentNodeKey, { pageToken: state.nextPageToken, append: true });
  }

  async function runSearch(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const query = searchQuery.trim();
    if (!query) return;
    cancelActiveSearch();
    const controller = new AbortController();
    searchAbortRef.current = controller;
    setSearchError(null);
    setSearchMatches([]);
    setSearchProgress(null);
    try {
      const result = await searchOpcTags.mutateAsync({
        bridgeHost,
        opcServer,
        query,
        matchMode: "contains",
        sessionId: sessionIdRef.current ?? undefined,
        maxResults: SEARCH_MAX_RESULTS,
        includeBranches: false,
        signal: controller.signal,
        onEvent: (searchEvent: OpcSearchEvent) => {
          if (controller.signal.aborted) return;
          if (searchEvent.type === "match") {
            setSearchMatches((previous) => [...previous, searchEvent.match]);
          } else if (searchEvent.type === "progress") {
            setSearchProgress(searchEvent.progress);
          } else {
            setSearchProgress(null);
          }
        },
      });
      if (controller.signal.aborted) return;
      setSearchMatches(result.matches);
      setSearchProgress(null);
      if (result.warning) {
        setSearchError(result.warning);
      } else if (result.truncated) {
        setSearchError(`Showing the first ${result.matches.length} matches.`);
      } else if (result.matches.length === 0) {
        setSearchError("No matching tags.");
      }
    } catch (err) {
      if (controller.signal.aborted) {
        setSearchError("Search cancelled.");
        return;
      }
      setSearchMatches([]);
      setSearchProgress(null);
      setSearchError(userFacingErrorMessage(err, "Unable to search tags."));
    } finally {
      if (searchAbortRef.current === controller) {
        searchAbortRef.current = null;
      }
    }
  }

  function chooseSearchMatch(match: OpcSearchMatchResponse) {
    setSearchError(null);
    void revealSearchMatch(match, () => false).then((revealed) => {
      if (!revealed) {
        const itemId = nodeItemId(match.node);
        if (itemId) setSelectedNode({ nodeKey: match.node.node_key, itemId });
      }
    });
  }

  const selectedTag = selectedNode?.itemId ?? null;
  const busy =
    testConnection.isPending ||
    selectionCheckPending ||
    searchOpcTags.isPending;

  return (
    <Modal
      title={
        qualityWarning
          ? "OPC quality warning"
          : `Browse tags on ${opcServer || "(no server)"}`
      }
      onClose={() => {
        disposeBrowse();
        onClose();
      }}
      widthClassName="max-w-2xl"
    >
      {qualityWarning ? (
        <div className="space-y-4">
          <div className="rounded-md border border-amber-800 bg-amber-950/50 p-3 text-sm text-amber-200">
            <p className="font-medium">
              This tag returned a non-Good OPC quality.
            </p>
            <p className="mt-2">
              The live value for{" "}
              <span className="font-mono">{qualityWarning.selectedTag}</span>{" "}
              was{" "}
              <span className="font-mono">{qualityWarning.reading.value}</span>{" "}
              with quality{" "}
              <Badge tone={SAMPLE_QUALITY_TONE[qualityWarning.reading.quality]}>
                {SAMPLE_QUALITY_LABELS[qualityWarning.reading.quality]}
              </Badge>
              .
            </p>
            <p className="mt-2">
              Non-Good values may be stale or invalid. Choose another tag, or
              proceed anyway if you understand the risk.
            </p>
            <p className="mt-2">
              Proceeding only selects this item for the form; a tune still
              requires trustworthy quality for its live readings.
            </p>
          </div>
          <div className="flex justify-end gap-2">
            <Button onClick={() => setQualityWarning(null)}>
              Choose a different tag
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                const tag = qualityWarning.selectedTag;
                setQualityWarning(null);
                applyTag(tag);
              }}
            >
              Proceed anyway
            </Button>
          </div>
        </div>
      ) : !opcServer ? (
        <p className="text-sm text-slate-400">
          Enter an OPC DA server ProgID above before browsing its tags.
        </p>
      ) : (
        <>
          <form className="mb-3 flex gap-2" onSubmit={runSearch}>
            <label className="sr-only" htmlFor="opc-tag-search">
              Search OPC tags
            </label>
            <input
              id="opc-tag-search"
              value={searchQuery}
              onChange={(event) => setSearchQuery(event.target.value)}
              placeholder="Search tags"
              className="min-w-0 flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 placeholder:text-slate-500"
            />
            {searchOpcTags.isPending ? (
              <Button type="button" onClick={cancelActiveSearch}>
                Cancel
              </Button>
            ) : (
              <Button type="submit" disabled={busy || !searchQuery.trim()}>
                Search
              </Button>
            )}
          </form>
          {(searchError || searchMatches.length > 0 || searchProgress) && (
            <div className="mb-3 max-h-32 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
              {searchProgress && (
                <p className="mb-2 text-xs text-slate-400">
                  Searching… visited {searchProgress.visited_nodes} nodes, found{" "}
                  {searchProgress.matches} matches
                  {searchProgress.partial ? " so far" : ""}.
                </p>
              )}
              {searchError && <ErrorBanner message={searchError} />}
              {searchMatches.length > 0 && (
                <div className="space-y-1">
                  {searchMatches.map((match) => {
                    const itemId = nodeItemId(match.node);
                    const breadcrumbs =
                      match.breadcrumbs.at(-1)?.node_key === match.node.node_key
                        ? match.breadcrumbs.slice(0, -1)
                        : match.breadcrumbs;
                    const path = [
                      ...breadcrumbs.map((part) => part.display_name),
                      match.node.display_name,
                    ].join(" / ");
                    return (
                      <button
                        key={match.node.node_key}
                        type="button"
                        disabled={busy || !itemId}
                        onClick={() => chooseSearchMatch(match)}
                        title={itemId ?? path}
                        className="block w-full truncate rounded px-2 py-1 text-left text-xs text-slate-300 hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        <span className="font-mono">{path}</span>
                        {itemId && itemId !== path && (
                          <span className="ml-2 text-slate-500">{itemId}</span>
                        )}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          )}

          <div className="max-h-64 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
            <TreeLevel
              parentNodeKey={null}
              depth={0}
              scopeState={scopeState}
              expanded={expanded}
              onToggle={toggle}
              onSelect={selectNode}
              onConfirm={(node) => {
                const itemId = nodeItemId(node);
                if (itemId) void confirmTag(itemId);
              }}
              onLoadMore={loadMore}
              selectedNode={selectedNode}
              selectedNodeRef={selectedNodeRef}
              disabled={busy}
            />
          </div>

          <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
            {!selectedTag ? (
              <p className="text-sm text-slate-400">
                Select a tag to test its live value and quality.
              </p>
            ) : (
              <>
                <p className="text-sm text-slate-200">
                  Selected: <span className="font-mono">{selectedTag}</span>
                </p>
                <p className="mt-1 text-xs text-slate-500">
                  Select tag applies the active template&apos;s process-variable
                  suffix. Review or override the rest of the mapping in the
                  collapsed section on the main tune form.
                </p>

                <div className="mt-3 flex items-center gap-2">
                  <Button disabled={busy} onClick={readSelectedTag}>
                    {selectionCheckPending
                      ? "Checking…"
                      : testConnection.isPending
                        ? "Reading…"
                        : "Read selected tag"}
                  </Button>
                  {testConnection.isSuccess && (
                    <span className="text-xs text-slate-300">
                      {testConnection.data.value}{" "}
                      <Badge
                        tone={SAMPLE_QUALITY_TONE[testConnection.data.quality]}
                      >
                        {SAMPLE_QUALITY_LABELS[testConnection.data.quality]}
                      </Badge>
                    </span>
                  )}
                  {testConnection.isError && !selectionReadError && (
                    <span className="text-xs text-red-400">
                      {userFacingErrorMessage(
                        testConnection.error,
                        "Unable to read the selected tag.",
                      )}
                    </span>
                  )}
                  {selectionReadError && (
                    <ErrorBanner message={selectionReadError} />
                  )}
                </div>

                <div className="mt-3 flex justify-end gap-2">
                  <Button
                    onClick={() => {
                      disposeBrowse();
                      onClose();
                    }}
                  >
                    Cancel
                  </Button>
                  <Button
                    variant="primary"
                    disabled={busy}
                    onClick={() => void confirmTag(selectedTag)}
                  >
                    {selectionCheckPending ? "Checking…" : "Select tag"}
                  </Button>
                </div>
              </>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}
