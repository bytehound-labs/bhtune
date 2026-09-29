import { useEffect, useLayoutEffect, useReducer } from "react";
import type { Dispatch, SetStateAction } from "react";
import {
  useOpcSearchIndexStatus,
  type OpcTagNodeResponse,
} from "../../api/opc";
import type { ScopeSnapshot, ScopeState, SelectedNode } from "./browseModel";
import { hasUsableIndex } from "./searchModel";
import { scrollSelectedNodeIntoView } from "./treeModel";
import {
  fallbackSelectedNode,
  initialRestorePhase,
  reduceRestorePhase,
  restoreLocateSettled,
  restoreOverlayPending,
  scrollSettleDecision,
} from "./restoreModel";

export function useSavedTagRestore({
  bridgeHost,
  opcServer,
  initialTag,
  load,
  revealInitialTag,
  disposeBrowse,
  searchIndexStatus,
  setExpanded,
  setSelectedNode,
  disposedRef,
  selectedNode,
  scopeState,
  expanded,
  selectedNodeRef,
  treeViewportRef,
}: {
  bridgeHost: string;
  opcServer: string;
  initialTag: string;
  load: (parentNodeKey: string | null) => Promise<ScopeSnapshot | null>;
  revealInitialTag: (
    rootNodes: OpcTagNodeResponse[],
    target: string,
    canUseIndexedSearch: boolean,
    isCancelled: () => boolean,
  ) => Promise<boolean>;
  disposeBrowse: () => void;
  searchIndexStatus: ReturnType<typeof useOpcSearchIndexStatus>;
  setExpanded: Dispatch<SetStateAction<Set<string>>>;
  setSelectedNode: (node: SelectedNode | null) => void;
  disposedRef: { current: boolean };
  selectedNode: SelectedNode | null;
  scopeState: Record<string, ScopeState>;
  expanded: Set<string>;
  selectedNodeRef: { current: HTMLDivElement | null };
  treeViewportRef: { current: HTMLDivElement | null };
}) {
  const [phase, dispatch] = useReducer(
    reduceRestorePhase,
    Boolean(opcServer),
    initialRestorePhase,
  );

  useEffect(() => {
    if (!opcServer) return;
    let cancelled = false;
    dispatch("start");
    disposedRef.current = false;
    async function initialize() {
      try {
        const root = await load(null);
        if (cancelled || !root) return;

        const target = initialTag;
        let revealStatus = searchIndexStatus.data;
        if (target && !revealStatus && !searchIndexStatus.isError) {
          revealStatus = (await searchIndexStatus.refetch()).data;
        }
        if (
          target &&
          (await revealInitialTag(
            root.nodes,
            target,
            hasUsableIndex(revealStatus),
            () => cancelled,
          ))
        ) {
          return;
        }

        if (!cancelled) {
          setExpanded(new Set());
          setSelectedNode(fallbackSelectedNode(root.nodes));
        }
      } finally {
        if (!cancelled) {
          dispatch("located");
        }
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

  useLayoutEffect(() => {
    if (!restoreOverlayPending(phase) || !restoreLocateSettled(phase)) return;
    if (!selectedNode) {
      dispatch("clear-overlay");
      return;
    }

    let frame = 0;
    let attempts = 0;
    const settle = () => {
      attempts += 1;
      const decision = scrollSettleDecision(
        scrollSelectedNodeIntoView(
          treeViewportRef.current,
          selectedNodeRef.current,
        ),
        attempts,
      );
      if (decision === "clear") {
        dispatch("clear-overlay");
        return;
      }
      if (decision === "retry") {
        frame = window.requestAnimationFrame(settle);
      }
    };
    frame = window.requestAnimationFrame(settle);
    return () => window.cancelAnimationFrame(frame);
    // The viewport and selected-row refs are stable composer refs. Reading
    // them here must not restart the settle loop.
  }, [
    phase,
    selectedNode,
    scopeState,
    expanded,
    treeViewportRef,
    selectedNodeRef,
  ]);

  return {
    initializationPending: restoreOverlayPending(phase),
  };
}
