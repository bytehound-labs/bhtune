import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type RefObject,
  type ReactNode,
} from "react";
import {
  useCloseOpcBrowseSession,
  useOpcBrowseFetcher,
  useOpcIndexedSearch,
  useOpcSearchIndexStatus,
  useControlOpcSearchIndex,
  useDeleteOpcSearchIndex,
  useRefreshOpcSearchIndex,
  useSetOpcSearchIndexAutoRefresh,
  useTestOpcConnection,
} from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import type {
  OpcBrowseResponse,
  OpcIndexedSearchMatchResponse,
  OpcReadResponse,
  OpcSearchIndexResponse,
  OpcSearchIndexStatusResponse,
  OpcTagNodeResponse,
} from "../api/opc";
import type { components } from "../api/schema";
import { SAMPLE_QUALITY_LABELS, SAMPLE_QUALITY_TONE } from "../lib/enumLabels";
import { deriveTag } from "../lib/opcTags";
import { formatExactTime, formatTimeUntil } from "../lib/time";
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
const SEARCH_MAX_RESULTS = 50;
const SEARCH_DEBOUNCE_MS = 150;

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

function searchStateLabel(status: OpcSearchIndexStatusResponse | undefined) {
  if (!status) return null;
  switch (status.state) {
    case "not_indexed":
      return "Not indexed";
    case "partial":
      return "Building";
    case "ready":
      return "Ready";
    case "stale":
      return "Stale";
    case "refreshing":
      return "Refreshing";
    case "failed":
      return "Failed";
    case "deleting":
      return "Deleting";
    default:
      return status.state;
  }
}

function hasUsableIndex(
  status: OpcSearchIndexStatusResponse | undefined,
): boolean {
  if (
    !status ||
    status.active_generation < 1 ||
    status.state === "partial" ||
    status.state === "not_indexed" ||
    status.state === "deleting"
  ) {
    return false;
  }
  return (
    status.state === "ready" ||
    status.state === "stale" ||
    status.state === "refreshing" ||
    status.state === "failed"
  );
}

function matchPath(match: OpcIndexedSearchMatchResponse): string {
  return [...match.breadcrumbs, match.display_name].join(" / ");
}

type TextRange = [number, number];

function searchTerms(query: string): string[] {
  return [...new Set(query.toLocaleLowerCase().split(/\s+/).filter(Boolean))];
}

function findTextRanges(text: string, terms: string[]): TextRange[] {
  const lowerText = text.toLocaleLowerCase();
  return terms.flatMap((term) => findRangesForTerm(lowerText, term));
}

function findRangesForTerm(lowerText: string, term: string): TextRange[] {
  const ranges: TextRange[] = [];
  let from = 0;
  while (from < lowerText.length) {
    const start = lowerText.indexOf(term, from);
    if (start < 0) break;
    ranges.push([start, start + term.length]);
    from = start + term.length;
  }
  return ranges;
}

function mergeTextRanges(ranges: TextRange[]): TextRange[] {
  const sorted = [...ranges].sort(([a], [b]) => a - b);
  const merged: TextRange[] = [];
  for (const [start, end] of sorted) {
    const previous = merged.at(-1);
    if (previous && start <= previous[1]) {
      previous[1] = Math.max(previous[1], end);
    } else {
      merged.push([start, end]);
    }
  }
  return merged;
}

function renderHighlightedParts(
  text: string,
  ranges: TextRange[],
): ReactNode[] {
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const [start, end] of ranges) {
    if (start > cursor) {
      parts.push(text.slice(cursor, start));
    }
    parts.push(
      <mark
        key={`${start}-${end}`}
        className="rounded bg-blue-900/70 px-0.5 text-blue-100"
      >
        {text.slice(start, end)}
      </mark>,
    );
    cursor = end;
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts;
}

function highlightedText(text: string, query: string): ReactNode {
  const terms = searchTerms(query);
  if (terms.length === 0) return text;
  const ranges = mergeTextRanges(findTextRanges(text, terms));
  return ranges.length === 0 ? text : renderHighlightedParts(text, ranges);
}

function indexUnavailableMessage(
  status: OpcSearchIndexStatusResponse | undefined,
  error: unknown,
): string {
  const suffix = " Lazy browse and direct ItemID entry remain available.";
  if (error) {
    return `Global search is unavailable: ${userFacingErrorMessage(
      error,
      "the gateway index status could not be read.",
    )}${suffix}`;
  }
  if (!status) {
    return `Global search is unavailable until the gateway index status is available.${suffix}`;
  }
  switch (status.state) {
    case "partial":
      return `Global search will be available when the gateway finishes building the index.${suffix}`;
    case "failed":
      return `Global search is unavailable because the gateway has no complete index.${suffix}`;
    case "deleting":
      return `The tag index is being deleted. Build a new index when deletion finishes.${suffix}`;
    default:
      return `Global search is unavailable until the gateway has a complete index.${suffix}`;
  }
}

function noSearchMatchesMessage(
  status: OpcSearchIndexStatusResponse | undefined,
  indexSearchAvailable: boolean,
  unavailableMessage: string,
): string {
  if (!indexSearchAvailable) return unavailableMessage;
  switch (status?.state) {
    case "partial":
      return "The tag index is still building; no complete no-match result is available yet.";
    case "not_indexed":
      return "The tag index has not been built. Build it to enable global search.";
    case "failed":
      return "The tag index failed to build. Retry it after resolving the gateway error.";
    case "deleting":
      return "The tag index is being deleted. Wait for deletion to finish before building a new index.";
    default:
      return "No matching tags.";
  }
}

function indexBuildButtonLabel(
  isPending: boolean,
  indexSearchAvailable: boolean,
  state: OpcSearchIndexStatusResponse["state"] | undefined,
): string {
  if (isPending) return "Building…";
  if (indexSearchAvailable) return "Refresh index";
  if (state === "failed") return "Retry build";
  return "Build index";
}

function autoRefreshButtonLabel(isPending: boolean, enabled: boolean): string {
  if (isPending) return "Saving…";
  return enabled ? "Disable auto-refresh" : "Enable auto-refresh";
}

function selectionReadButtonLabel(
  selectionCheckPending: boolean,
  readPending: boolean,
): string {
  if (selectionCheckPending) return "Checking…";
  if (readPending) return "Reading…";
  return "Read selected tag";
}

function autoRefreshErrorMessage(enabled: boolean): string {
  if (enabled) return "Unable to enable automatic index refresh.";
  return "Unable to disable automatic index refresh.";
}

function nextSearchIndex(
  previous: number,
  direction: 1 | -1,
  resultCount: number,
): number {
  let start = previous;
  if (previous < 0) {
    start = direction > 0 ? 0 : resultCount - 1;
  }
  return (start + direction + resultCount) % resultCount;
}

function handleTreeNodeDoubleClick(
  node: OpcTagNodeResponse,
  onToggle: (node: OpcTagNodeResponse) => void,
  onConfirm: (node: OpcTagNodeResponse) => void,
): void {
  if (nodeCanExpand(node)) {
    onToggle(node);
    return;
  }
  if (nodeItemId(node)) {
    onConfirm(node);
    return;
  }
  onToggle(node);
}

type TreeLevelProps = Readonly<{
  parentNodeKey: string | null;
  depth: number;
  scopeState: Record<string, ScopeState>;
  expanded: Set<string>;
  onToggle: (node: OpcTagNodeResponse) => void;
  onSelect: (node: OpcTagNodeResponse) => void;
  onConfirm: (node: OpcTagNodeResponse) => void;
  onLoadMore: (parentNodeKey: string | null) => void;
  onRetry: (parentNodeKey: string | null) => void;
  selectedNode: SelectedNode | null;
  selectedNodeRef: RefObject<HTMLButtonElement | null>;
  disabled: boolean;
}>;

type TreeNodeRowProps = Readonly<{
  node: OpcTagNodeResponse;
  depth: number;
  expanded: Set<string>;
  onToggle: (node: OpcTagNodeResponse) => void;
  onSelect: (node: OpcTagNodeResponse) => void;
  onConfirm: (node: OpcTagNodeResponse) => void;
  scopeState: Record<string, ScopeState>;
  onLoadMore: (parentNodeKey: string | null) => void;
  onRetry: (parentNodeKey: string | null) => void;
  selectedNode: SelectedNode | null;
  selectedNodeRef: RefObject<HTMLButtonElement | null>;
  disabled: boolean;
}>;

function TreeNodeRow({
  node,
  depth,
  expanded,
  onToggle,
  onSelect,
  onConfirm,
  scopeState,
  onLoadMore,
  onRetry,
  selectedNode,
  selectedNodeRef,
  disabled,
}: TreeNodeRowProps) {
  const isBranch = nodeCanExpand(node);
  const itemId = nodeItemId(node);
  const isSelected = selectedNode?.nodeKey === node.node_key;
  const isExpanded = expanded.has(node.node_key);
  const expandLabel = isExpanded ? "Collapse" : "Expand";
  const expandGlyph = isExpanded ? "▾" : "▸";
  const rowClassName = `flex items-center gap-1.5 rounded px-1 py-1 text-sm hover:bg-slate-800 ${
    isSelected ? "bg-slate-800" : ""
  }`;
  const itemButtonRef = isSelected ? selectedNodeRef : undefined;
  const itemButtonDisabled = disabled || (!itemId && !isBranch);
  const itemButtonTitle = itemId ?? node.display_name;
  const kindLabel = nodeKindLabel(node);

  function selectOrToggle() {
    if (itemId) {
      onSelect(node);
      return;
    }
    onToggle(node);
  }

  return (
    <div key={node.node_key}>
      <div
        className={rowClassName}
        style={{ paddingLeft: `${depth * INDENT_PX}px` }}
      >
        {isBranch ? (
          <button
            type="button"
            onClick={() => onToggle(node)}
            disabled={disabled}
            aria-label={expandLabel}
            className="w-4 shrink-0 text-slate-400 hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {expandGlyph}
          </button>
        ) : (
          <span className="w-4 shrink-0" />
        )}
        <button
          type="button"
          onClick={selectOrToggle}
          onDoubleClick={() =>
            handleTreeNodeDoubleClick(node, onToggle, onConfirm)
          }
          ref={itemButtonRef}
          disabled={itemButtonDisabled}
          title={itemButtonTitle}
          className="flex-1 truncate text-left font-mono text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {node.display_name}
        </button>
        {kindLabel && (
          <span className="shrink-0 text-xs text-slate-500">{kindLabel}</span>
        )}
      </div>
      {isBranch && isExpanded && (
        <TreeLevel
          parentNodeKey={node.node_key}
          depth={depth + 1}
          scopeState={scopeState}
          expanded={expanded}
          onToggle={onToggle}
          onSelect={onSelect}
          onConfirm={onConfirm}
          onLoadMore={onLoadMore}
          onRetry={onRetry}
          selectedNode={selectedNode}
          selectedNodeRef={selectedNodeRef}
          disabled={disabled}
        />
      )}
    </div>
  );
}

function TreeLevelError({
  depth,
  message,
  onRetry,
  parentNodeKey,
  disabled,
}: Readonly<{
  depth: number;
  message: string | undefined;
  onRetry: (parentNodeKey: string | null) => void;
  parentNodeKey: string | null;
  disabled: boolean;
}>) {
  return (
    <div
      className="flex items-center gap-2 py-1 text-xs text-red-400"
      style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
    >
      <span>{message}</span>
      <button
        type="button"
        onClick={() => onRetry(parentNodeKey)}
        disabled={disabled}
        className="text-blue-300 underline hover:text-blue-200 disabled:cursor-not-allowed disabled:opacity-50"
      >
        Retry
      </button>
    </div>
  );
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
  onRetry,
  selectedNode,
  selectedNodeRef,
  disabled,
}: TreeLevelProps) {
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
      <TreeLevelError
        depth={depth}
        message={state.message}
        onRetry={onRetry}
        parentNodeKey={parentNodeKey}
        disabled={disabled}
      />
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
      {state.nodes.map((node) => (
        <TreeNodeRow
          key={node.node_key}
          node={node}
          depth={depth}
          expanded={expanded}
          onToggle={onToggle}
          onSelect={onSelect}
          onConfirm={onConfirm}
          scopeState={scopeState}
          onLoadMore={onLoadMore}
          onRetry={onRetry}
          selectedNode={selectedNode}
          selectedNodeRef={selectedNodeRef}
          disabled={disabled}
        />
      ))}
      {state.status === "error" && state.nodes.length > 0 && (
        <TreeLevelError
          depth={depth}
          message={state.message}
          onRetry={onRetry}
          parentNodeKey={parentNodeKey}
          disabled={disabled}
        />
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

type QualityWarningPanelProps = Readonly<{
  warning: QualityWarning;
  onChooseDifferent: () => void;
  onProceed: () => void;
}>;

function QualityWarningPanel({
  warning,
  onChooseDifferent,
  onProceed,
}: QualityWarningPanelProps) {
  return (
    <div className="space-y-4">
      <div className="rounded-md border border-amber-800 bg-amber-950/50 p-3 text-sm text-amber-200">
        <p className="font-medium">This tag returned a non-Good OPC quality.</p>
        <p className="mt-2">
          The live value for{" "}
          <span className="font-mono">{warning.selectedTag}</span> was{" "}
          <span className="font-mono">{warning.reading.value}</span> with
          quality{" "}
          <Badge tone={SAMPLE_QUALITY_TONE[warning.reading.quality]}>
            {SAMPLE_QUALITY_LABELS[warning.reading.quality]}
          </Badge>
          .
        </p>
        <p className="mt-2">
          Non-Good values may be stale or invalid. Choose another tag, or
          proceed anyway if you understand the risk.
        </p>
        <p className="mt-2">
          Proceeding only selects this item for the form; a tune still requires
          trustworthy quality for its live readings.
        </p>
      </div>
      <div className="flex justify-end gap-2">
        <Button onClick={onChooseDifferent}>Choose a different tag</Button>
        <Button variant="primary" onClick={onProceed}>
          Proceed anyway
        </Button>
      </div>
    </div>
  );
}

type IndexControlsProps = Readonly<{
  opcServer: string;
  indexStatus: OpcSearchIndexStatusResponse | undefined;
  indexStateLabel: string | null;
  indexSearchAvailable: boolean;
  indexUnavailableMessage: string;
  refreshPending: boolean;
  controlPending: boolean;
  autoRefreshPending: boolean;
  deletePending: boolean;
  onRefresh: () => void;
  onCancel: () => void;
  onSetAutoRefresh: (enabled: boolean) => void;
  onDelete: () => void;
}>;

function IndexControls({
  opcServer,
  indexStatus,
  indexStateLabel,
  indexSearchAvailable,
  indexUnavailableMessage,
  refreshPending,
  controlPending,
  autoRefreshPending,
  deletePending,
  onRefresh,
  onCancel,
  onSetAutoRefresh,
  onDelete,
}: IndexControlsProps) {
  const canCancelBuild =
    indexStatus?.state === "partial" || indexStatus?.state === "refreshing";
  const canDelete =
    indexStatus &&
    (indexStatus.active_generation > 0 || indexStatus.state === "failed");
  const deleteDisabled =
    deletePending ||
    refreshPending ||
    indexStatus?.state === "partial" ||
    indexStatus?.state === "refreshing";

  return (
    <div className="mb-3 space-y-2">
      <div className="flex gap-2">
        <Button
          type="button"
          disabled={
            refreshPending ||
            !opcServer ||
            indexStatus?.state === "partial" ||
            indexStatus?.state === "refreshing" ||
            indexStatus?.state === "deleting"
          }
          onClick={onRefresh}
        >
          {indexBuildButtonLabel(
            refreshPending,
            indexSearchAvailable,
            indexStatus?.state,
          )}
        </Button>
        {canCancelBuild && (
          <Button type="button" disabled={controlPending} onClick={onCancel}>
            {controlPending ? "Cancelling…" : "Cancel build"}
          </Button>
        )}
        {indexStatus?.state === "deleting" && (
          <output className="text-xs text-amber-300">
            Deleting the tag index… browse and direct reads remain available.
          </output>
        )}
        {canDelete && (
          <>
            {indexStatus.active_generation > 0 && (
              <Button
                type="button"
                disabled={autoRefreshPending || deletePending}
                onClick={() =>
                  onSetAutoRefresh(!indexStatus.auto_refresh_enabled)
                }
              >
                {autoRefreshButtonLabel(
                  autoRefreshPending,
                  indexStatus.auto_refresh_enabled,
                )}
              </Button>
            )}
            <Button
              type="button"
              variant="danger"
              disabled={deleteDisabled}
              onClick={onDelete}
            >
              {deletePending ? "Deleting…" : "Delete index"}
            </Button>
          </>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-500">
        {indexStateLabel && (
          <span className="text-slate-400">
            Index: {indexStateLabel.toLocaleLowerCase()}
          </span>
        )}
        {indexStatus?.progress && (
          <span>
            {indexStatus.progress.entries_seen.toLocaleString()} entries{" · "}
            {indexStatus.progress.items_per_second.toFixed(0)} items/s
          </span>
        )}
        {indexStatus?.scheduler.next_refresh_at && (
          <span
            title={
              formatExactTime(indexStatus.scheduler.next_refresh_at) ??
              undefined
            }
          >
            Next refresh:{" "}
            {formatTimeUntil(indexStatus.scheduler.next_refresh_at)}
          </span>
        )}
        {indexStatus && indexStatus.active_generation > 0 && (
          <span>
            Auto-refresh:{" "}
            {indexStatus.auto_refresh_enabled ? "enabled" : "disabled"}
          </span>
        )}
      </div>
      {indexStatus?.state === "failed" && indexStatus.last_error && (
        <output className="text-xs text-red-300">
          Index error: {indexStatus.last_error}
        </output>
      )}
      {!indexSearchAvailable && (
        <output className="text-xs text-slate-400">
          {indexUnavailableMessage}
        </output>
      )}
    </div>
  );
}

type IndexedSearchResultsProps = Readonly<{
  searchError: string | null;
  searchMatches: OpcIndexedSearchMatchResponse[];
  searchResponse: OpcSearchIndexResponse | null;
  searchQuery: string;
  indexStatus: OpcSearchIndexStatusResponse | undefined;
  indexSearchAvailable: boolean;
  indexUnavailableMessage: string;
  searchPending: boolean;
  busy: boolean;
  activeSearchIndex: number;
  onResultRef: (index: number, element: HTMLButtonElement | null) => void;
  onHover: (index: number) => void;
  onSelect: (match: OpcIndexedSearchMatchResponse) => void;
  onConfirm: (match: OpcIndexedSearchMatchResponse) => void;
}>;

function IndexedSearchResults({
  searchError,
  searchMatches,
  searchResponse,
  searchQuery,
  indexStatus,
  indexSearchAvailable,
  indexUnavailableMessage,
  searchPending,
  busy,
  activeSearchIndex,
  onResultRef,
  onHover,
  onSelect,
  onConfirm,
}: IndexedSearchResultsProps) {
  const query = searchQuery.trim();
  const shouldRender =
    Boolean(searchError) ||
    searchMatches.length > 0 ||
    query.length >= 2 ||
    Boolean(indexStatus?.progress);
  if (!shouldRender) return null;

  return (
    <div className="mb-3 max-h-56 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
      {searchError && <ErrorBanner message={searchError} />}
      {searchPending && (
        <p className="mb-2 text-xs text-slate-400">
          Searching… previous results remain visible until the new query
          completes.
        </p>
      )}
      {searchMatches.length > 0 && (
        <div
          role="listbox"
          aria-label="OPC tag search results"
          className="space-y-1"
        >
          {searchMatches.map((match, index) => {
            const path = matchPath(match);
            const active = index === activeSearchIndex;
            return (
              <button
                key={match.item_id}
                id={`opc-search-result-${index}`}
                ref={(element) => onResultRef(index, element)}
                role="option"
                aria-selected={active}
                type="button"
                disabled={busy}
                onMouseEnter={() => onHover(index)}
                onClick={() => onSelect(match)}
                onDoubleClick={() => onConfirm(match)}
                title={match.item_id}
                className={`block w-full rounded px-2 py-1.5 text-left text-xs disabled:cursor-not-allowed disabled:opacity-50 ${
                  active
                    ? "bg-blue-950/70 text-blue-100"
                    : "text-slate-300 hover:bg-slate-800"
                }`}
              >
                <span className="block truncate font-mono">
                  {highlightedText(match.item_id, searchQuery)}
                </span>
                <span className="block truncate text-slate-500">
                  {highlightedText(path, searchQuery)}
                </span>
              </button>
            );
          })}
        </div>
      )}
      {searchResponse?.has_more && (
        <p className="mt-2 text-xs text-slate-400">
          50+ matches — keep typing to narrow.
        </p>
      )}
      {!searchPending &&
        !searchError &&
        query.length >= 2 &&
        searchMatches.length === 0 && (
          <p className="text-xs text-slate-400">
            {noSearchMatchesMessage(
              indexStatus,
              indexSearchAvailable,
              indexUnavailableMessage,
            )}
          </p>
        )}
    </div>
  );
}

type TestConnectionState = ReturnType<typeof useTestOpcConnection>;

type SelectedTagPanelProps = Readonly<{
  selectedTag: string | null;
  busy: boolean;
  selectionCheckPending: boolean;
  selectionReadError: string | null;
  testConnection: TestConnectionState;
  onRead: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}>;

function SelectedTagPanel({
  selectedTag,
  busy,
  selectionCheckPending,
  selectionReadError,
  testConnection,
  onRead,
  onCancel,
  onConfirm,
}: SelectedTagPanelProps) {
  return (
    <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
      {!selectedTag && (
        <p className="text-sm text-slate-400">
          Select a tag to test its live value and quality.
        </p>
      )}
      {selectedTag && (
        <>
          <p className="text-sm text-slate-200">
            Selected: <span className="font-mono">{selectedTag}</span>
          </p>
          <p className="mt-1 text-xs text-slate-500">
            Select tag applies the active template&apos;s process-variable
            suffix. Review or override the rest of the mapping in the collapsed
            section on the main tune form.
          </p>

          <div className="mt-3 flex items-center gap-2">
            <Button disabled={busy} onClick={onRead}>
              {selectionReadButtonLabel(
                selectionCheckPending,
                testConnection.isPending,
              )}
            </Button>
            {testConnection.isSuccess && testConnection.data && (
              <span className="text-xs text-slate-300">
                {testConnection.data.value}{" "}
                <Badge tone={SAMPLE_QUALITY_TONE[testConnection.data.quality]}>
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
            {selectionReadError && <ErrorBanner message={selectionReadError} />}
          </div>

          <div className="mt-3 flex justify-end gap-2">
            <Button onClick={onCancel}>Cancel</Button>
            <Button variant="primary" disabled={busy} onClick={onConfirm}>
              {selectionCheckPending ? "Checking…" : "Select tag"}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

type TagBrowserContentProps = Readonly<{
  indexControls: IndexControlsProps;
  searchResults: IndexedSearchResultsProps;
  tree: TreeLevelProps;
  selectedTagPanel: SelectedTagPanelProps;
}>;

function TagBrowserContent({
  indexControls,
  searchResults,
  tree,
  selectedTagPanel,
}: TagBrowserContentProps) {
  return (
    <>
      <IndexControls {...indexControls} />
      <IndexedSearchResults {...searchResults} />
      <div className="max-h-64 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
        <TreeLevel {...tree} />
      </div>
      <SelectedTagPanel {...selectedTagPanel} />
    </>
  );
}

/**
 * The OPC tag-tree browser modal (`ui-opc-browser`): a lazily-expanding, paged tree fed by
 * `GET /api/opc/browse`, a per-node "Read selected tag" action backed by `GET /api/opc/read`,
 * and an incremental search backed by the gateway-owned persistent index. Browse navigation
 * round-trips the gateway's opaque session, node, and page tokens; display names are never
 * parsed into paths. When the user confirms a selection, the active template's
 * process-variable suffix is applied to the selected node's exact original ItemID, after a
 * fresh quality check reads that same ItemID. Reopening at a saved tag uses indexed-search
 * breadcrumbs to reveal and scroll the matching node when the server supports it.
 */
export function OpcTagBrowserModal({
  bridgeHost,
  opcServer,
  template,
  initialTag,
  onClose,
  onSelect,
}: Readonly<{
  bridgeHost: string;
  opcServer: string;
  template: TemplateResponse | undefined;
  initialTag: string;
  onClose: () => void;
  onSelect: (tag: string) => void;
}>) {
  const { fetchPage, clearCache } = useOpcBrowseFetcher(bridgeHost, opcServer);
  const closeBrowseSession = useCloseOpcBrowseSession();
  const indexedSearch = useOpcIndexedSearch();
  const searchIndexStatus = useOpcSearchIndexStatus(
    bridgeHost,
    opcServer,
    Boolean(opcServer),
  );
  const refreshSearchIndex = useRefreshOpcSearchIndex();
  const controlSearchIndex = useControlOpcSearchIndex();
  const setAutoRefreshMutation = useSetOpcSearchIndexAutoRefresh();
  const deleteSearchIndex = useDeleteOpcSearchIndex();
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
  const [searchMatches, setSearchMatches] = useState<
    OpcIndexedSearchMatchResponse[]
  >([]);
  const [searchResponse, setSearchResponse] =
    useState<OpcSearchIndexResponse | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [activeSearchIndex, setActiveSearchIndex] = useState(-1);
  const searchAbortRef = useRef<AbortController | null>(null);
  const searchResultRefs = useRef<Record<number, HTMLButtonElement | null>>({});
  const indexStatus = searchIndexStatus.data ?? searchResponse?.status;
  const indexStateLabel = searchStateLabel(indexStatus);
  const indexSearchAvailable = hasUsableIndex(indexStatus);
  const unavailableMessage = indexUnavailableMessage(
    indexStatus,
    searchIndexStatus.error,
  );

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

  async function ensureNodeByName(
    parentNodeKey: string | null,
    displayName: string,
    expectedItemId: string | null,
    isCancelled: () => boolean,
  ): Promise<OpcTagNodeResponse | null> {
    let state = scopeStateRef.current[scopeKey(parentNodeKey)];
    if (!state) {
      const snapshot = await load(parentNodeKey);
      if (!snapshot || isCancelled()) return null;
      state = { status: "loaded", ...snapshot };
    }

    while (!isCancelled()) {
      const match = state.nodes.find((node) => {
        return (
          node.display_name === displayName &&
          (!expectedItemId || nodeItemId(node) === expectedItemId)
        );
      });
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

  async function revealIndexedSearchMatch(
    match: OpcIndexedSearchMatchResponse,
    isCancelled: () => boolean,
  ): Promise<boolean> {
    let parentNodeKey: string | null = null;
    const breadcrumbs =
      match.breadcrumbs.at(-1) === match.display_name
        ? match.breadcrumbs.slice(0, -1)
        : match.breadcrumbs;

    for (const breadcrumb of breadcrumbs) {
      const branch = await ensureNodeByName(
        parentNodeKey,
        breadcrumb,
        null,
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

    const node = await ensureNodeByName(
      parentNodeKey,
      match.display_name,
      match.item_id,
      isCancelled,
    );
    if (!node || !nodeItemId(node)) return false;
    setSelectedNode({ nodeKey: node.node_key, itemId: match.item_id });
    return true;
  }

  async function revealInitialTag(
    rootNodes: OpcTagNodeResponse[],
    target: string,
    canUseIndexedSearch: boolean,
    isCancelled: () => boolean,
  ): Promise<boolean> {
    const rootMatch = rootNodes.find((node) => nodeItemId(node) === target);
    if (rootMatch) {
      setSelectedNode({ nodeKey: rootMatch.node_key, itemId: target });
      return true;
    }
    if (!canUseIndexedSearch) return false;

    const controller = new AbortController();
    searchAbortRef.current = controller;
    try {
      const result = await indexedSearch.mutateAsync({
        bridgeHost,
        opcServer,
        query: target,
        matchMode: "exact",
        maxResults: 1,
        signal: controller.signal,
      });
      const match = result.matches.find(
        (candidate) => candidate.item_id === target,
      );
      if (!match || isCancelled() || controller.signal.aborted) return false;
      return revealIndexedSearchMatch(match, isCancelled);
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
    disposedRef.current = false;
    async function initialize() {
      const root = await load(null);
      if (cancelled || !root) return;

      const target = initialTag;
      let revealStatus = searchIndexStatus.data;
      if (target && !revealStatus && !searchIndexStatus.isError) {
        revealStatus = (await searchIndexStatus.refetch()).data;
      }
      if (
        target &&
        hasUsableIndex(revealStatus) &&
        (await revealInitialTag(root.nodes, target, true, () => cancelled))
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
    setSearchMatches([]);
    setSearchResponse(null);
    setSearchError(null);
    setActiveSearchIndex(-1);
  }, [bridgeHost, opcServer]);

  useEffect(() => {
    if (indexStatus?.state !== "deleting") return;
    searchAbortRef.current?.abort();
    setSearchMatches([]);
    setSearchResponse(null);
    setSearchError(null);
    setActiveSearchIndex(-1);
  }, [indexStatus?.state]);

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
    let pvTag = tag;
    if (template) {
      pvTag = deriveTag(tag, template.process_variable_suffix) ?? tag;
    }
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

  function retryBrowse(parentNodeKey: string | null) {
    const state = scopeStateRef.current[scopeKey(parentNodeKey)];
    if (!state) return;
    if (state.nodes.length > 0 && state.nextPageToken) {
      void load(parentNodeKey, {
        pageToken: state.nextPageToken,
        append: true,
      });
    } else {
      void load(parentNodeKey);
    }
  }

  useEffect(() => {
    const query = searchQuery.trim();
    cancelActiveSearch();
    setSearchError(null);
    setActiveSearchIndex(-1);

    if (query.length < 2 || !opcServer || !indexSearchAvailable) {
      setSearchMatches([]);
      setSearchResponse(null);
      return;
    }

    const controller = new AbortController();
    searchAbortRef.current = controller;
    const timer = window.setTimeout(async () => {
      if (controller.signal.aborted) return;
      try {
        const result = await indexedSearch.mutateAsync({
          bridgeHost,
          opcServer,
          query,
          matchMode: query.length < 3 ? "prefix" : "contains",
          maxResults: SEARCH_MAX_RESULTS,
          signal: controller.signal,
        });
        if (
          controller.signal.aborted ||
          searchAbortRef.current !== controller
        ) {
          return;
        }
        setSearchResponse(result);
        setSearchMatches(result.matches);
        setActiveSearchIndex(result.matches.length > 0 ? 0 : -1);
      } catch (err) {
        if (controller.signal.aborted) return;
        setSearchError(userFacingErrorMessage(err, "Unable to search tags."));
        setSearchResponse(null);
        setSearchMatches([]);
      } finally {
        if (searchAbortRef.current === controller) {
          searchAbortRef.current = null;
        }
      }
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      window.clearTimeout(timer);
      controller.abort();
      if (searchAbortRef.current === controller) {
        searchAbortRef.current = null;
      }
    };
    // `indexedSearch` is a stable mutation hook; changing connection or query
    // intentionally starts a new debounced request.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [searchQuery, bridgeHost, opcServer, indexSearchAvailable]);

  useEffect(() => {
    if (activeSearchIndex < 0) return;
    searchResultRefs.current[activeSearchIndex]?.scrollIntoView({
      block: "nearest",
    });
  }, [activeSearchIndex]);

  function chooseSearchMatch(match: OpcIndexedSearchMatchResponse) {
    setSearchError(null);
    setSelectionReadError(null);
    testConnection.reset();
    setSelectedNode({
      nodeKey: `indexed:${match.item_id}`,
      itemId: match.item_id,
    });
  }

  function handleSearchKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      setSearchQuery("");
      return;
    }
    if (searchMatches.length === 0) return;

    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const direction = event.key === "ArrowDown" ? 1 : -1;
      setActiveSearchIndex((previous) =>
        nextSearchIndex(previous, direction, searchMatches.length),
      );
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      setActiveSearchIndex(event.key === "Home" ? 0 : searchMatches.length - 1);
    } else if (event.key === "Enter" && activeSearchIndex >= 0) {
      event.preventDefault();
      chooseSearchMatch(searchMatches[activeSearchIndex]);
    }
  }

  const selectedTag = selectedNode?.itemId ?? null;
  const busy = testConnection.isPending || selectionCheckPending;

  async function refreshIndex() {
    setSearchError(null);
    try {
      const status = await refreshSearchIndex.mutateAsync({
        bridgeHost,
        opcServer,
        force: true,
      });
      setSearchResponse((previous) =>
        previous ? { ...previous, status } : previous,
      );
      await searchIndexStatus.refetch();
    } catch (err) {
      setSearchError(
        userFacingErrorMessage(err, "Unable to refresh the tag index."),
      );
    }
  }

  async function setAutoRefresh(enabled: boolean) {
    setSearchError(null);
    try {
      await setAutoRefreshMutation.mutateAsync({
        bridgeHost,
        opcServer,
        enabled,
      });
      await searchIndexStatus.refetch();
    } catch (err) {
      setSearchError(
        userFacingErrorMessage(err, autoRefreshErrorMessage(enabled)),
      );
    }
  }

  async function deleteIndex() {
    if (
      !window.confirm(
        `Delete the namespace index for ${opcServer}? Search data and enrollment will be removed.`,
      )
    ) {
      return;
    }

    setSearchError(null);
    try {
      await deleteSearchIndex.mutateAsync({ bridgeHost, opcServer });
      setSearchMatches([]);
      setSearchResponse(null);
      await searchIndexStatus.refetch();
    } catch (err) {
      setSearchError(
        userFacingErrorMessage(err, "Unable to delete the tag index."),
      );
    }
  }

  async function cancelIndexBuild() {
    setSearchError(null);
    try {
      await controlSearchIndex.mutateAsync({
        bridgeHost,
        opcServer,
        action: "cancel",
      });
      await searchIndexStatus.refetch();
    } catch (err) {
      setSearchError(
        userFacingErrorMessage(err, "Unable to cancel the tag-index build."),
      );
    }
  }

  useEffect(() => {
    if (
      indexStatus?.state !== "partial" &&
      indexStatus?.state !== "refreshing" &&
      indexStatus?.state !== "deleting"
    ) {
      return;
    }
    const interval = window.setInterval(() => {
      void searchIndexStatus.refetch();
    }, 1_000);
    return () => window.clearInterval(interval);
    // `refetch` is the same query operation for this modal; depending on its
    // render-time identity would restart the interval on every query update.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [indexStatus?.state]);

  function confirmNode(node: OpcTagNodeResponse) {
    const itemId = nodeItemId(node);
    if (itemId) void confirmTag(itemId);
  }

  function closeFromSelection() {
    disposeBrowse();
    onClose();
  }

  function proceedWithQualityWarning() {
    const tag = qualityWarning?.selectedTag;
    if (!tag) return;
    setQualityWarning(null);
    applyTag(tag);
  }

  const modalTitle = qualityWarning
    ? "OPC quality warning"
    : `Browse tags on ${opcServer || "(no server)"}`;
  let modalContent: ReactNode;
  if (qualityWarning) {
    modalContent = (
      <QualityWarningPanel
        warning={qualityWarning}
        onChooseDifferent={() => setQualityWarning(null)}
        onProceed={proceedWithQualityWarning}
      />
    );
  } else if (!opcServer) {
    modalContent = (
      <p className="text-sm text-slate-400">
        Enter an OPC DA server ProgID above before browsing its tags.
      </p>
    );
  } else {
    modalContent = (
      <>
        <div className="mb-3 flex gap-2">
          <label className="sr-only" htmlFor="opc-tag-search">
            Search OPC tags
          </label>
          <input
            id="opc-tag-search"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={handleSearchKeyDown}
            disabled={!indexSearchAvailable}
            placeholder={
              indexSearchAvailable
                ? "Type at least 2 characters to search tags"
                : "Global search unavailable — browse below or enter an ItemID"
            }
            aria-activedescendant={
              activeSearchIndex >= 0
                ? `opc-search-result-${activeSearchIndex}`
                : undefined
            }
            className="min-w-0 flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 placeholder:text-slate-500"
          />
          {indexedSearch.isPending && (
            <Button type="button" onClick={cancelActiveSearch}>
              Cancel
            </Button>
          )}
        </div>
        <TagBrowserContent
          indexControls={{
            opcServer,
            indexStatus,
            indexStateLabel,
            indexSearchAvailable,
            indexUnavailableMessage: unavailableMessage,
            refreshPending: refreshSearchIndex.isPending,
            controlPending: controlSearchIndex.isPending,
            autoRefreshPending: setAutoRefreshMutation.isPending,
            deletePending: deleteSearchIndex.isPending,
            onRefresh: () => void refreshIndex(),
            onCancel: () => void cancelIndexBuild(),
            onSetAutoRefresh: (enabled) => void setAutoRefresh(enabled),
            onDelete: () => void deleteIndex(),
          }}
          searchResults={{
            searchError,
            searchMatches,
            searchResponse,
            searchQuery,
            indexStatus,
            indexSearchAvailable,
            indexUnavailableMessage: unavailableMessage,
            searchPending: indexedSearch.isPending,
            busy,
            activeSearchIndex,
            onResultRef: (index, element) => {
              searchResultRefs.current[index] = element;
            },
            onHover: setActiveSearchIndex,
            onSelect: chooseSearchMatch,
            onConfirm: (match) => void confirmTag(match.item_id),
          }}
          tree={{
            parentNodeKey: null,
            depth: 0,
            scopeState,
            expanded,
            onToggle: toggle,
            onSelect: selectNode,
            onConfirm: confirmNode,
            onLoadMore: loadMore,
            onRetry: retryBrowse,
            selectedNode,
            selectedNodeRef,
            disabled: busy,
          }}
          selectedTagPanel={{
            selectedTag,
            busy,
            selectionCheckPending,
            selectionReadError,
            testConnection,
            onRead: readSelectedTag,
            onCancel: closeFromSelection,
            onConfirm: () => {
              if (selectedTag) void confirmTag(selectedTag);
            },
          }}
        />
      </>
    );
  }

  return (
    <Modal
      title={modalTitle}
      onClose={closeFromSelection}
      widthClassName="max-w-2xl"
      documentationId="new-tune.opc-tag-browser"
    >
      {modalContent}
    </Modal>
  );
}
