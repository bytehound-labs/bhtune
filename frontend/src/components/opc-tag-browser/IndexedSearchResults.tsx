import type { ReactNode } from "react";
import type {
  OpcIndexedSearchMatchResponse,
  OpcSearchIndexResponse,
  OpcSearchIndexStatusResponse,
} from "../../api/opc";
import { ErrorBanner } from "../ui";
import {
  highlightedRanges,
  matchPath,
  noSearchMatchesMessage,
} from "./searchModel";

function renderHighlightedParts(
  text: string,
  ranges: ReturnType<typeof highlightedRanges>,
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
  const ranges = highlightedRanges(text, query);
  return ranges.length === 0 ? text : renderHighlightedParts(text, ranges);
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

export function IndexedSearchResults({
  searchError,
  searchMatches,
  searchResponse,
  searchQuery,
  indexStatus,
  indexSearchAvailable,
  indexUnavailableMessage: unavailableMessage,
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
              unavailableMessage,
            )}
          </p>
        )}
    </div>
  );
}
