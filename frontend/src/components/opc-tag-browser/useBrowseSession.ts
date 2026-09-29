import { useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { userFacingErrorMessage } from "../../api/errors";
import {
  searchOpcLive,
  useCloseOpcBrowseSession,
  useOpcBrowseFetcher,
  useOpcIndexedSearch,
} from "../../api/opc";
import type { OpcTagNodeResponse } from "../../api/opc";
import {
  BROWSE_PAGE_SIZE,
  MAX_ROOT_SCOPE_PAGES,
  ROOT_SCOPE_KEY,
  mergePage,
  nodeCanExpand,
  nodeItemId,
  scopeKey,
} from "./browseModel";
import type { ScopeSnapshot, ScopeState, SelectedNode } from "./browseModel";
import {
  revealMatchFromIndexedSearch,
  revealMatchFromLiveSearch,
  rootScopeCandidates,
} from "./restoreModel";
import type { RevealSearchMatch } from "./restoreModel";

export function useBrowseSession({
  bridgeHost,
  opcServer,
  disposedRef,
  searchAbortRef,
  indexedSearch,
  fetchPage,
  clearCache,
  closeBrowseSession,
  setExpanded,
  setSelectedNode,
}: {
  bridgeHost: string;
  opcServer: string;
  disposedRef: { current: boolean };
  searchAbortRef: { current: AbortController | null };
  indexedSearch: ReturnType<typeof useOpcIndexedSearch>;
  fetchPage: ReturnType<typeof useOpcBrowseFetcher>["fetchPage"];
  clearCache: ReturnType<typeof useOpcBrowseFetcher>["clearCache"];
  closeBrowseSession: ReturnType<typeof useCloseOpcBrowseSession>;
  setExpanded: Dispatch<SetStateAction<Set<string>>>;
  setSelectedNode: Dispatch<SetStateAction<SelectedNode | null>>;
}) {
  const [scopeState, setScopeState] = useState<Record<string, ScopeState>>({});
  const scopeStateRef = useRef(scopeState);
  const sessionIdRef = useRef<string | null>(null);
  const closedSessionIdsRef = useRef<Set<string>>(new Set());

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
    expectedNodeKey: string | null,
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
          (!expectedItemId || nodeItemId(node) === expectedItemId) &&
          (!expectedNodeKey || node.node_key === expectedNodeKey)
        );
      });
      if (match) return match;
      if (!state.nextPageToken) return null;
      // oxlint-disable-next-line no-await-in-loop -- Read each continuation page before requesting the next token.
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
    match: RevealSearchMatch,
    isCancelled: () => boolean,
  ): Promise<boolean> {
    let parentNodeKey: string | null = null;
    const breadcrumbs =
      match.breadcrumbs.at(-1)?.displayName === match.displayName
        ? match.breadcrumbs.slice(0, -1)
        : match.breadcrumbs;

    // oxlint-disable no-await-in-loop -- Each child lookup depends on the preceding breadcrumb.
    for (const breadcrumb of breadcrumbs) {
      const branch = await ensureNodeByName(
        parentNodeKey,
        breadcrumb.displayName,
        null,
        breadcrumb.nodeKey,
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
    // oxlint-enable no-await-in-loop

    const node = await ensureNodeByName(
      parentNodeKey,
      match.displayName,
      match.itemId,
      match.nodeKey,
      isCancelled,
    );
    if (!node || !nodeItemId(node)) return false;
    setSelectedNode({ nodeKey: node.node_key, itemId: match.itemId });
    return true;
  }

  async function revealRootScopeCandidate(
    candidate: OpcTagNodeResponse,
    target: string,
    isCancelled: () => boolean,
    controller: AbortController,
  ): Promise<boolean> {
    try {
      const liveMatches = await searchOpcLive({
        bridgeHost,
        opcServer,
        query: target,
        sessionId: sessionIdRef.current ?? undefined,
        scopeNodeKey: candidate.node_key,
        maxResults: 1,
        signal: controller.signal,
      });
      const liveMatch = liveMatches.find(
        (match) => match.node.item_id === target,
      );
      if (!liveMatch || isCancelled() || controller.signal.aborted) {
        return false;
      }
      const revealMatch = revealMatchFromLiveSearch(liveMatch, candidate);
      return revealMatch ? revealSearchMatch(revealMatch, isCancelled) : false;
    } catch {
      return false;
    }
  }

  async function revealWithinRootScopes(
    initialRootNodes: OpcTagNodeResponse[],
    target: string,
    isCancelled: () => boolean,
    controller: AbortController,
  ): Promise<{ revealed: boolean; foundScope: boolean }> {
    let state = scopeStateRef.current[ROOT_SCOPE_KEY] ?? {
      status: "loaded" as const,
      nodes: initialRootNodes,
      nextPageToken: null,
      complete: true,
      warning: null,
    };
    const attemptedScopes = new Set<string>();
    let pagesRead = 0;

    while (!isCancelled() && !controller.signal.aborted) {
      const candidates = rootScopeCandidates(state.nodes, target).filter(
        (candidate) => !attemptedScopes.has(candidate.node_key),
      );
      // oxlint-disable no-await-in-loop -- Root-scope searches stop at the first exact reveal.
      for (const candidate of candidates) {
        if (isCancelled() || controller.signal.aborted) break;
        attemptedScopes.add(candidate.node_key);
        if (
          await revealRootScopeCandidate(
            candidate,
            target,
            isCancelled,
            controller,
          )
        ) {
          return { revealed: true, foundScope: true };
        }
      }
      // oxlint-enable no-await-in-loop

      if (!state.nextPageToken || pagesRead >= MAX_ROOT_SCOPE_PAGES) {
        return {
          revealed: false,
          foundScope: attemptedScopes.size > 0,
        };
      }
      // oxlint-disable-next-line no-await-in-loop -- The next root page token comes from this page.
      const snapshot = await load(null, {
        pageToken: state.nextPageToken,
        append: true,
      });
      if (!snapshot) {
        return {
          revealed: false,
          foundScope: attemptedScopes.size > 0,
        };
      }
      state = { status: "loaded", ...snapshot };
      pagesRead += 1;
    }

    return { revealed: false, foundScope: attemptedScopes.size > 0 };
  }

  async function revealFromIndexedSearch(
    target: string,
    isCancelled: () => boolean,
    controller: AbortController,
  ): Promise<boolean> {
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
      if (!match || isCancelled() || controller.signal.aborted) {
        return false;
      }
      return revealSearchMatch(
        revealMatchFromIndexedSearch(match),
        isCancelled,
      );
    } catch {
      return false;
    }
  }

  async function revealFromUnscopedLiveSearch(
    target: string,
    isCancelled: () => boolean,
    controller: AbortController,
  ): Promise<boolean> {
    try {
      const liveMatches = await searchOpcLive({
        bridgeHost,
        opcServer,
        query: target,
        sessionId: sessionIdRef.current ?? undefined,
        maxResults: 1,
        signal: controller.signal,
      });
      const liveMatch = liveMatches.find(
        (candidate) => candidate.node.item_id === target,
      );
      if (!liveMatch || isCancelled() || controller.signal.aborted) {
        return false;
      }
      const revealMatch = revealMatchFromLiveSearch(liveMatch);
      return revealMatch ? revealSearchMatch(revealMatch, isCancelled) : false;
    } catch {
      return false;
    }
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

    const controller = new AbortController();
    searchAbortRef.current = controller;
    try {
      if (
        canUseIndexedSearch &&
        (await revealFromIndexedSearch(target, isCancelled, controller))
      ) {
        return true;
      }

      if (isCancelled() || controller.signal.aborted) return false;
      const scoped = await revealWithinRootScopes(
        rootNodes,
        target,
        isCancelled,
        controller,
      );
      if (scoped.revealed || scoped.foundScope) return scoped.revealed;

      return revealFromUnscopedLiveSearch(target, isCancelled, controller);
    } catch {
      return false;
    } finally {
      if (searchAbortRef.current === controller) {
        searchAbortRef.current = null;
      }
    }
  }

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

  return {
    scopeState,
    load,
    revealInitialTag,
    disposeBrowse,
    toggle,
    loadMore,
    retryBrowse,
  };
}
