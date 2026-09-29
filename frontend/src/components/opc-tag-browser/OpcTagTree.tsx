import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent as ReactMouseEvent,
  RefObject,
} from "react";
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
  selectedNodeRef: RefObject<HTMLDivElement | null>;
  activeNodeKey: string | null;
  onActiveNodeChange: (nodeKey: string) => void;
  disabled: boolean;
}>;

type TreeNodeRowProps = Readonly<{
  node: OpcTagNodeResponse;
  parentNodeKey: string | null;
  depth: number;
  position: number;
  setSize: number;
  expanded: Set<string>;
  onToggle: (node: OpcTagNodeResponse) => void;
  onSelect: (node: OpcTagNodeResponse) => void;
  onConfirm: (node: OpcTagNodeResponse) => void;
  scopeState: Record<string, ScopeState>;
  onLoadMore: (parentNodeKey: string | null) => void;
  onRetry: (parentNodeKey: string | null) => void;
  selectedNode: SelectedNode | null;
  selectedNodeRef: RefObject<HTMLDivElement | null>;
  activeNodeKey: string | null;
  onActiveNodeChange: (nodeKey: string) => void;
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
      role="alert"
      className="flex items-center gap-2 py-1 text-xs text-red-400"
      style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
    >
      <span>{message}</span>
      <button
        type="button"
        onClick={() => onRetry(parentNodeKey)}
        disabled={disabled}
        className="text-blue-300 underline hover:text-blue-200 disabled:cursor-not-allowed"
      >
        Retry
      </button>
    </div>
  );
}

function isTreeItemRowTarget(event: ReactMouseEvent<HTMLDivElement>): boolean {
  const target = event.target;
  return (
    target instanceof Element &&
    target.closest('[role="treeitem"]') === event.currentTarget &&
    !target.closest("button, a, input, select, textarea")
  );
}

function visibleTreeNodeKeys(
  scopeState: Record<string, ScopeState>,
  expanded: Set<string>,
  parentNodeKey: string | null = null,
): string[] {
  const state = scopeState[scopeKey(parentNodeKey)];
  if (!state) return [];

  return state.nodes.flatMap((node) =>
    [node.node_key].concat(
      expanded.has(node.node_key)
        ? visibleTreeNodeKeys(scopeState, expanded, node.node_key)
        : [],
    ),
  );
}

function focusTreeItem(
  tree: HTMLElement,
  nodeKey: string,
  onActiveNodeChange: (nodeKey: string) => void,
): void {
  const item = Array.from(
    tree.querySelectorAll<HTMLElement>('[role="treeitem"]'),
  ).find((element) => element.dataset.treeNodeKey === nodeKey);
  if (!item) return;
  onActiveNodeChange(nodeKey);
  item.focus();
}

function TreeNodeRow({
  node,
  parentNodeKey,
  depth,
  position,
  setSize,
  expanded,
  onToggle,
  onSelect,
  onConfirm,
  scopeState,
  onLoadMore,
  onRetry,
  selectedNode,
  selectedNodeRef,
  activeNodeKey,
  onActiveNodeChange,
  disabled,
}: TreeNodeRowProps) {
  const isBranch = nodeCanExpand(node);
  const itemId = nodeItemId(node);
  const isSelected = selectedNode?.nodeKey === node.node_key;
  const isExpanded = expanded.has(node.node_key);
  const expandGlyph = isExpanded ? "▾" : "▸";
  const childScope = scopeState[scopeKey(node.node_key)];
  const childLoading =
    isExpanded &&
    (childScope?.status === "loading" || childScope?.status === "loading-more");
  const rowClassName = `flex items-center gap-1.5 rounded px-1 py-1 text-sm hover:bg-slate-800 ${
    isSelected ? "bg-slate-800" : ""
  }`;
  const kindLabel = nodeKindLabel(node);

  function handleClick(event: ReactMouseEvent<HTMLDivElement>) {
    if (!isTreeItemRowTarget(event)) return;
    event.currentTarget.focus();
    onActiveNodeChange(node.node_key);
    if (disabled) return;
    if (itemId) onSelect(node);
    else onToggle(node);
  }

  function handleDoubleClick(event: ReactMouseEvent<HTMLDivElement>) {
    if (!isTreeItemRowTarget(event)) return;
    event.currentTarget.focus();
    onActiveNodeChange(node.node_key);
    if (!disabled) {
      handleTreeNodeDoubleClick(node, onToggle, onConfirm);
    }
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.target !== event.currentTarget) return;
    const tree = event.currentTarget.closest<HTMLElement>('[role="tree"]');
    if (!tree) return;
    const visibleItems = Array.from(
      tree.querySelectorAll<HTMLElement>('[role="treeitem"]'),
    );
    const currentIndex = visibleItems.indexOf(event.currentTarget);
    const parentKey = event.currentTarget.dataset.treeParentNodeKey;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (currentIndex >= 0 && currentIndex < visibleItems.length - 1) {
          const next = visibleItems[currentIndex + 1];
          if (next) {
            onActiveNodeChange(next.dataset.treeNodeKey ?? "");
            next.focus();
          }
        }
        break;
      case "ArrowUp":
        event.preventDefault();
        if (currentIndex > 0) {
          const previous = visibleItems[currentIndex - 1];
          if (previous) {
            onActiveNodeChange(previous.dataset.treeNodeKey ?? "");
            previous.focus();
          }
        }
        break;
      case "Home": {
        event.preventDefault();
        const first = visibleItems[0];
        if (first) {
          onActiveNodeChange(first.dataset.treeNodeKey ?? "");
          first.focus();
        }
        break;
      }
      case "End": {
        event.preventDefault();
        const last = visibleItems.at(-1);
        if (last) {
          onActiveNodeChange(last.dataset.treeNodeKey ?? "");
          last.focus();
        }
        break;
      }
      case "ArrowRight":
        event.preventDefault();
        if (!isBranch) break;
        if (!isExpanded) {
          if (!disabled) onToggle(node);
          break;
        }
        {
          const child = visibleItems.find(
            (item) => item.dataset.treeParentNodeKey === node.node_key,
          );
          if (child) {
            onActiveNodeChange(child.dataset.treeNodeKey ?? "");
            child.focus();
          }
        }
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (isBranch && isExpanded && !disabled) {
          onToggle(node);
        } else if (parentKey) {
          focusTreeItem(tree, parentKey, onActiveNodeChange);
        }
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        if (disabled) break;
        if (itemId) onSelect(node);
        else if (isBranch) onToggle(node);
        break;
      default:
        break;
    }
  }

  return (
    <div
      ref={isSelected ? selectedNodeRef : undefined}
      role="treeitem"
      aria-label={node.display_name}
      aria-level={depth + 1}
      aria-posinset={position}
      aria-setsize={setSize}
      aria-expanded={isBranch ? isExpanded : undefined}
      aria-selected={itemId ? isSelected : undefined}
      aria-disabled={disabled || (!itemId && !isBranch) || undefined}
      aria-busy={childLoading || undefined}
      data-tree-node-key={node.node_key}
      data-tree-parent-node-key={parentNodeKey ?? ""}
      tabIndex={activeNodeKey === node.node_key ? 0 : -1}
      onFocus={() => onActiveNodeChange(node.node_key)}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
      onKeyDown={handleKeyDown}
      className={disabled ? "cursor-not-allowed" : "cursor-pointer"}
    >
      <div
        className={rowClassName}
        style={{ paddingLeft: `${depth * INDENT_PX}px` }}
      >
        <span aria-hidden="true" className="w-4 shrink-0 text-slate-400">
          {isBranch ? expandGlyph : ""}
        </span>
        <span className="flex-1 truncate font-mono text-slate-200">
          {node.display_name}
        </span>
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
          activeNodeKey={activeNodeKey}
          onActiveNodeChange={onActiveNodeChange}
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
  activeNodeKey,
  onActiveNodeChange,
  disabled,
}: TreeLevelProps) {
  const state = scopeState[scopeKey(parentNodeKey)];
  if (!state) return null;

  const visibleKeys =
    depth === 0 ? visibleTreeNodeKeys(scopeState, expanded) : undefined;
  const activeKey =
    depth === 0 && visibleKeys
      ? activeNodeKey && visibleKeys.includes(activeNodeKey)
        ? activeNodeKey
        : selectedNode?.nodeKey && visibleKeys.includes(selectedNode.nodeKey)
          ? selectedNode.nodeKey
          : (visibleKeys[0] ?? null)
      : activeNodeKey;

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
          role="status"
          className="py-1 text-xs text-amber-300"
          style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
        >
          {state.warning}
        </div>
      )}
      <div
        role={depth === 0 ? "tree" : "group"}
        aria-label={depth === 0 ? "OPC tag hierarchy" : undefined}
        aria-busy={
          state.status === "loading" ||
          state.status === "loading-more" ||
          undefined
        }
        aria-disabled={disabled || undefined}
        className="space-y-1"
      >
        {state.nodes.map((node, index) => (
          <TreeNodeRow
            key={node.node_key}
            node={node}
            parentNodeKey={parentNodeKey}
            depth={depth}
            position={index + 1}
            setSize={state.complete ? state.nodes.length : -1}
            expanded={expanded}
            onToggle={onToggle}
            onSelect={onSelect}
            onConfirm={onConfirm}
            scopeState={scopeState}
            onLoadMore={onLoadMore}
            onRetry={onRetry}
            selectedNode={selectedNode}
            selectedNodeRef={selectedNodeRef}
            activeNodeKey={activeKey}
            onActiveNodeChange={onActiveNodeChange}
            disabled={disabled}
          />
        ))}
      </div>
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
          className="py-1 text-xs text-blue-300 hover:text-blue-200 disabled:cursor-not-allowed"
          style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
        >
          {state.status === "loading-more" && <Spinner size="sm" />}
          {state.status === "loading-more" ? "Loading more…" : "Load more"}
        </button>
      )}
    </>
  );
}
