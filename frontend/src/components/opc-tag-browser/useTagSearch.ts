import { useEffect, useRef, useState } from "react";
import type { Dispatch, KeyboardEvent, SetStateAction } from "react";
import { userFacingErrorMessage } from "../../api/errors";
import {
  useControlOpcSearchIndex,
  useDeleteOpcSearchIndex,
  useOpcIndexedSearch,
  useOpcSearchIndexStatus,
  useRefreshOpcSearchIndex,
  useSetOpcSearchIndexAutoRefresh,
  useTestOpcConnection,
} from "../../api/opc";
import type {
  OpcIndexedSearchMatchResponse,
  OpcSearchIndexResponse,
} from "../../api/opc";
import type { SelectedNode } from "./browseModel";
import {
  SEARCH_DEBOUNCE_MS,
  SEARCH_MAX_RESULTS,
  autoRefreshErrorMessage,
  hasUsableIndex,
  indexUnavailableMessage,
  nextSearchIndex,
  searchMatchMode,
  searchStateLabel,
} from "./searchModel";

export function useTagSearch({
  bridgeHost,
  opcServer,
  indexedSearch,
  searchIndexStatus,
  refreshSearchIndex,
  controlSearchIndex,
  setAutoRefreshMutation,
  deleteSearchIndex,
  searchAbortRef,
  setSelectedNode,
  setSelectionReadError,
  testConnection,
}: {
  bridgeHost: string;
  opcServer: string;
  indexedSearch: ReturnType<typeof useOpcIndexedSearch>;
  searchIndexStatus: ReturnType<typeof useOpcSearchIndexStatus>;
  refreshSearchIndex: ReturnType<typeof useRefreshOpcSearchIndex>;
  controlSearchIndex: ReturnType<typeof useControlOpcSearchIndex>;
  setAutoRefreshMutation: ReturnType<typeof useSetOpcSearchIndexAutoRefresh>;
  deleteSearchIndex: ReturnType<typeof useDeleteOpcSearchIndex>;
  searchAbortRef: { current: AbortController | null };
  setSelectedNode: (node: SelectedNode | null) => void;
  setSelectionReadError: Dispatch<SetStateAction<string | null>>;
  testConnection: ReturnType<typeof useTestOpcConnection>;
}) {
  const [searchQuery, setSearchQuery] = useState("");
  const [searchMatches, setSearchMatches] = useState<
    OpcIndexedSearchMatchResponse[]
  >([]);
  const [searchResponse, setSearchResponse] =
    useState<OpcSearchIndexResponse | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [deleteConfirmationOpen, setDeleteConfirmationOpen] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [activeSearchIndex, setActiveSearchIndex] = useState(-1);
  const searchResultRefs = useRef<Record<number, HTMLButtonElement | null>>({});
  const indexStatus = searchIndexStatus.data ?? searchResponse?.status;
  const indexStateLabel = searchStateLabel(indexStatus);
  const indexSearchAvailable = hasUsableIndex(indexStatus);
  const unavailableMessage = indexUnavailableMessage(
    indexStatus,
    searchIndexStatus.error,
  );

  function cancelActiveSearch() {
    searchAbortRef.current?.abort();
  }

  function setSearchResultElement(
    index: number,
    element: HTMLButtonElement | null,
  ) {
    searchResultRefs.current[index] = element;
  }

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
    // `searchAbortRef` is a stable composer ref. Depending on its current
    // controller would clear search state on unrelated renders.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [indexStatus?.state]);

  useEffect(() => {
    const query = searchQuery.trim();
    setSearchError(null);
    setActiveSearchIndex(-1);

    if (query.length < 2 || !opcServer || !indexSearchAvailable) {
      setSearchMatches([]);
      setSearchResponse(null);
      return;
    }

    // Preserve saved-tag restoration when only index availability changes.
    cancelActiveSearch();
    const controller = new AbortController();
    searchAbortRef.current = controller;
    const timer = window.setTimeout(async () => {
      if (controller.signal.aborted) return;
      try {
        const result = await indexedSearch.mutateAsync({
          bridgeHost,
          opcServer,
          query,
          matchMode: searchMatchMode(query),
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
      const match = searchMatches[activeSearchIndex];
      if (match) chooseSearchMatch(match);
    }
  }

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

  function requestDeleteIndex() {
    deleteSearchIndex.reset();
    setDeleteError(null);
    setDeleteConfirmationOpen(true);
  }

  function cancelDeleteIndex() {
    if (deleteSearchIndex.isPending) return;
    deleteSearchIndex.reset();
    setDeleteError(null);
    setDeleteConfirmationOpen(false);
  }

  async function confirmDeleteIndex() {
    if (deleteSearchIndex.isPending) return;
    setDeleteError(null);
    try {
      await deleteSearchIndex.mutateAsync({ bridgeHost, opcServer });
      setSearchMatches([]);
      setSearchResponse(null);
      await searchIndexStatus.refetch();
      setDeleteConfirmationOpen(false);
    } catch (err) {
      setDeleteError(
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

  return {
    searchQuery,
    setSearchQuery,
    searchMatches,
    searchResponse,
    searchError,
    deleteConfirmationOpen,
    deleteError,
    activeSearchIndex,
    setActiveSearchIndex,
    setSearchResultElement,
    indexStatus,
    indexStateLabel,
    indexSearchAvailable,
    unavailableMessage,
    cancelActiveSearch,
    handleSearchKeyDown,
    chooseSearchMatch,
    refreshIndex,
    setAutoRefresh,
    requestDeleteIndex,
    cancelDeleteIndex,
    confirmDeleteIndex,
    cancelIndexBuild,
  };
}
