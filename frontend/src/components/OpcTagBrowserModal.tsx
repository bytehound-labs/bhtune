import { useCallback, useId, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { components } from "../api/schema";
import { Button, ConfirmModal, Modal } from "./ui";
import { QualityWarningPanel } from "./opc-tag-browser/QualityWarningPanel";
import { TagBrowserContent } from "./opc-tag-browser/TagBrowserContent";
import { useBrowseSession } from "./opc-tag-browser/useBrowseSession";
import { savedTagInitializationMessage } from "./opc-tag-browser/restoreModel";
import type { SelectedNode } from "./opc-tag-browser/browseModel";
import { useSavedTagRestore } from "./opc-tag-browser/useSavedTagRestore";
import { useTagSearch } from "./opc-tag-browser/useTagSearch";
import { useTagSelection } from "./opc-tag-browser/useTagSelection";
import { indexErrorIdentity } from "./opc-tag-browser/searchModel";
import {
  useCloseOpcBrowseSession,
  useControlOpcSearchIndex,
  useDeleteOpcSearchIndex,
  useOpcBrowseFetcher,
  useOpcIndexedSearch,
  useOpcSearchIndexStatus,
  useRefreshOpcSearchIndex,
  useSetOpcSearchIndexAutoRefresh,
  useTestOpcConnection,
} from "../api/opc";

type TemplateResponse = components["schemas"]["TemplateResponse"];

/**
 * The OPC tag-tree browser modal (`ui-opc-browser`): a lazily-expanding, paged tree fed by
 * `GET /api/opc/browse`, a per-node "Read selected tag" action backed by `GET /api/opc/read`,
 * and an incremental search backed by the gateway-owned persistent index. Browse navigation
 * round-trips the gateway's opaque session, node, and page tokens; display names are never
 * parsed into paths. When the user confirms a selection, the active template's
 * process-variable suffix is applied to the selected node's exact original ItemID, after a
 * fresh quality check reads that same ItemID. Reopening at a saved tag uses persistent
 * indexed-search breadcrumbs when available and falls back to a bounded live search before
 * selecting an unrelated root item.
 */
export function OpcTagBrowserModal({
  bridgeHost,
  opcServer,
  template,
  initialTag,
  acknowledgedIndexErrors,
  onIndexBuildStarted,
  onClose,
  onSelect,
}: Readonly<{
  bridgeHost: string;
  opcServer: string;
  template: TemplateResponse | undefined;
  initialTag: string;
  acknowledgedIndexErrors: readonly string[];
  onIndexBuildStarted: () => void;
  onClose: (shownIndexErrors: readonly string[]) => void;
  onSelect: (tag: string) => void;
}>) {
  const shownIndexErrors = useRef(new Set<string>());
  const rearmIndexErrors = useCallback(() => {
    // Closing during a new build must not acknowledge an older identical error.
    shownIndexErrors.current.clear();
    onIndexBuildStarted();
  }, [onIndexBuildStarted]);
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
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedNode, setSelectedNode] = useState<SelectedNode | null>(null);
  const [activeTreeNodeKey, setActiveTreeNodeKey] = useState<string | null>(
    null,
  );
  const setSelectedNodeAndActiveTreeNode = useCallback(
    (node: SelectedNode | null) => {
      setSelectedNode(node);
      setActiveTreeNodeKey(node?.nodeKey ?? null);
    },
    [],
  );
  const selectedNodeRef = useRef<HTMLDivElement | null>(null);
  const treeViewportRef = useRef<HTMLDivElement | null>(null);
  const searchInputId = useId();
  const searchResultsId = useId();
  const disposedRef = useRef(false);
  const searchAbortRef = useRef<AbortController | null>(null);
  const browse = useBrowseSession({
    bridgeHost,
    opcServer,
    disposedRef,
    searchAbortRef,
    indexedSearch,
    fetchPage,
    clearCache,
    closeBrowseSession,
    setExpanded,
    setSelectedNode: setSelectedNodeAndActiveTreeNode,
  });
  const restore = useSavedTagRestore({
    bridgeHost,
    opcServer,
    initialTag,
    load: browse.load,
    revealInitialTag: browse.revealInitialTag,
    disposeBrowse: browse.disposeBrowse,
    searchIndexStatus,
    setExpanded,
    setSelectedNode: setSelectedNodeAndActiveTreeNode,
    disposedRef,
    selectedNode,
    scopeState: browse.scopeState,
    expanded,
    selectedNodeRef,
    treeViewportRef,
  });
  const selection = useTagSelection({
    bridgeHost,
    opcServer,
    template,
    onClose: () => onClose([...shownIndexErrors.current]),
    onSelect,
    disposeBrowse: browse.disposeBrowse,
    testConnection,
    selectedNode,
    setSelectedNode: setSelectedNodeAndActiveTreeNode,
  });
  const search = useTagSearch({
    bridgeHost,
    opcServer,
    indexedSearch,
    searchIndexStatus,
    refreshSearchIndex,
    controlSearchIndex,
    setAutoRefreshMutation,
    deleteSearchIndex,
    searchAbortRef,
    setSelectedNode: setSelectedNodeAndActiveTreeNode,
    setSelectionReadError: selection.setSelectionReadError,
    testConnection,
    onIndexBuildStarted: rearmIndexErrors,
  });
  const { scopeState, toggle, loadMore, retryBrowse } = browse;
  const { initializationPending } = restore;
  const {
    qualityWarning,
    setQualityWarning,
    selectionCheckPending,
    selectionReadError,
    selectNode,
    confirmTag,
    confirmNode,
    readSelectedTag,
    closeFromSelection,
    proceedWithQualityWarning,
  } = selection;
  const {
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
  } = search;
  const indexErrorId = indexErrorIdentity(indexStatus);
  const indexError =
    indexErrorId && !acknowledgedIndexErrors.includes(indexErrorId)
      ? (indexStatus?.last_error ?? null)
      : null;
  const selectedTag = selectedNode?.itemId ?? null;
  const busy = testConnection.isPending || selectionCheckPending;
  const initializationMessage = savedTagInitializationMessage(initialTag);
  const activeSearchResultId =
    activeSearchIndex >= 0 && searchMatches[activeSearchIndex]
      ? `${searchResultsId}-result-${activeSearchIndex}`
      : undefined;

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
          <label className="sr-only" htmlFor={searchInputId}>
            Search OPC tags
          </label>
          <input
            id={searchInputId}
            role="combobox"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={handleSearchKeyDown}
            disabled={!indexSearchAvailable}
            aria-autocomplete="list"
            aria-haspopup="listbox"
            aria-expanded={searchMatches.length > 0}
            aria-controls={
              searchMatches.length > 0 ? searchResultsId : undefined
            }
            placeholder={
              indexSearchAvailable
                ? "Type at least 2 characters to search tags"
                : "Global search unavailable — browse below or enter an ItemID"
            }
            aria-activedescendant={activeSearchResultId}
            className="min-w-0 flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 placeholder:text-slate-500"
          />
          {indexedSearch.isPending && (
            <Button type="button" loading onClick={cancelActiveSearch}>
              Cancel
            </Button>
          )}
        </div>
        <TagBrowserContent
          indexControls={{
            opcServer,
            indexStatus,
            indexStateLabel,
            indexError,
            onIndexErrorShown: (element) => {
              if (element && indexErrorId && !deleteConfirmationOpen) {
                shownIndexErrors.current.add(indexErrorId);
              }
            },
            indexSearchAvailable,
            indexUnavailableMessage: unavailableMessage,
            refreshPending: refreshSearchIndex.isPending,
            controlPending: controlSearchIndex.isPending,
            autoRefreshPending: setAutoRefreshMutation.isPending,
            deletePending: deleteSearchIndex.isPending,
            onRefresh: () => void refreshIndex(),
            onCancel: () => void cancelIndexBuild(),
            onSetAutoRefresh: (enabled) => void setAutoRefresh(enabled),
            onDelete: requestDeleteIndex,
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
            listboxId: searchResultsId,
            onResultRef: setSearchResultElement,
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
            activeNodeKey: activeTreeNodeKey,
            onActiveNodeChange: setActiveTreeNodeKey,
            disabled: busy,
          }}
          treeViewportRef={treeViewportRef}
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
          initializationPending={initializationPending}
          initializationMessage={initializationMessage}
        />
      </>
    );
  }

  return (
    <>
      <Modal
        title={modalTitle}
        onClose={() => {
          if (!deleteConfirmationOpen) closeFromSelection();
        }}
        dismissible={!deleteConfirmationOpen}
        widthClassName="max-w-2xl"
        documentationId="new-tune.opc-tag-browser"
      >
        {modalContent}
      </Modal>
      {deleteConfirmationOpen && (
        <ConfirmModal
          title="Delete tag index?"
          onCancel={cancelDeleteIndex}
          onConfirm={() => void confirmDeleteIndex()}
          pending={deleteSearchIndex.isPending}
          confirmLabel="Delete index"
          pendingLabel="Deleting index…"
          errorMessage={deleteError}
          documentationId="new-tune.opc-tag-browser.delete-confirmation"
        >
          <p>
            Delete the namespace index for <strong>{opcServer}</strong>?
          </p>
          <p className="mt-2 text-slate-400">
            Indexed search data and this server&apos;s index enrollment will be
            removed. Lazy browsing, direct ItemID entry, live reads, and tuning
            remain available.
          </p>
        </ConfirmModal>
      )}
    </>
  );
}
