import type { RefObject } from "react";
import type { OpcTagNodeResponse } from "../../api/opc";
import { LoadingStatus, Spinner } from "../ui";
import type { ScopeState, SelectedNode } from "./browseModel";
import {
  nodeCanExpand,
  nodeItemId,
  nodeKindLabel,
  scopeKey,
} from "./browseModel";
import { handleTreeNodeDoubleClick } from "./treeModel";

/** Indentation step per tree depth; matches the width of the expand chevron column so a
 * leaf's label lines up under its parent branch's label, not under its chevron. */
const INDENT_PX = 18;

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
        <OpcTagTree
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

/** One tree level -- renders one browsed scope and recurses into whichever branch nodes are
 * expanded. Navigation uses only the gateway's opaque `node_key`; `item_id` is kept only for
 * reads/selections. */
export function OpcTagTree({
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
      <LoadingStatus
        message="Loading…"
        size="sm"
        className="py-1 text-xs text-slate-500"
      />
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
          {state.status === "loading-more" && <Spinner size="sm" />}
          {state.status === "loading-more" ? "Loading more…" : "Load more"}
        </button>
      )}
    </>
  );
}
