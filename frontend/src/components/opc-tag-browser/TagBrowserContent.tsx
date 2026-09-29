import type { ComponentProps, RefObject } from "react";
import { LoadingOverlay } from "../ui";
import { IndexControls } from "./IndexControls";
import { IndexedSearchResults } from "./IndexedSearchResults";
import { OpcTagTree } from "./OpcTagTree";
import { SelectedTagPanel } from "./SelectedTagPanel";

export function TagBrowserContent({
  indexControls,
  searchResults,
  tree,
  treeViewportRef,
  selectedTagPanel,
  initializationPending,
  initializationMessage,
}: Readonly<{
  indexControls: ComponentProps<typeof IndexControls>;
  searchResults: ComponentProps<typeof IndexedSearchResults>;
  tree: ComponentProps<typeof OpcTagTree>;
  treeViewportRef: RefObject<HTMLDivElement | null>;
  selectedTagPanel: ComponentProps<typeof SelectedTagPanel>;
  initializationPending: boolean;
  initializationMessage: string;
}>) {
  return (
    <>
      <IndexControls {...indexControls} />
      <IndexedSearchResults {...searchResults} />
      <LoadingOverlay
        active={initializationPending}
        message={initializationMessage}
        className="mt-3 min-h-[24rem]"
      >
        <div
          ref={treeViewportRef}
          data-testid="opc-tag-tree-viewport"
          className="max-h-64 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2"
        >
          <OpcTagTree {...tree} />
        </div>
        <SelectedTagPanel {...selectedTagPanel} />
      </LoadingOverlay>
    </>
  );
}
