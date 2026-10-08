import { userFacingErrorMessage } from "../../api/errors";
import type {
  OpcIndexedSearchMatchResponse,
  OpcSearchIndexStatusResponse,
} from "../../api/opc";

export const SEARCH_MAX_RESULTS = 50;

export const SEARCH_DEBOUNCE_MS = 150;

export function searchStateLabel(
  status: OpcSearchIndexStatusResponse | undefined,
) {
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

export function hasUsableIndex(
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

export function indexErrorIdentity(
  status: OpcSearchIndexStatusResponse | undefined,
): string | null {
  if (status?.state !== "failed" || !status.last_error) return null;
  return JSON.stringify([
    status.started_at ?? null,
    status.completed_at ?? null,
    status.scheduler.last_attempt_at ?? null,
    status.last_error,
  ]);
}

export function matchPath(match: OpcIndexedSearchMatchResponse): string {
  return [...match.breadcrumbs, match.display_name].join(" / ");
}

export function indexUnavailableMessage(
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

export function noSearchMatchesMessage(
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

export function indexBuildButtonLabel(
  indexSearchAvailable: boolean,
  state: OpcSearchIndexStatusResponse["state"] | undefined,
): string {
  if (indexSearchAvailable) return "Refresh index";
  if (state === "failed") return "Retry build";
  return "Build index";
}

export function autoRefreshButtonLabel(
  status: OpcSearchIndexStatusResponse,
): string {
  if (status.auto_refresh_enabled) {
    return status.scheduler.auto_refresh_policy === "allowed" &&
      status.scheduler.next_refresh_at
      ? "Disable auto-refresh"
      : "Disable server preference";
  }
  return status.scheduler.auto_refresh_policy
    ? "Enable auto-refresh"
    : "Enable server preference";
}

export function autoRefreshPolicyMessage(
  policy: OpcSearchIndexStatusResponse["scheduler"]["auto_refresh_policy"],
): string | null {
  switch (policy) {
    case "allowed":
      return null;
    case "disabled":
      return "Automatic refresh is blocked by gateway configuration (index.enabled = false). Cached search and manual refresh remain available.";
    case "paused":
      return "Automatic refresh is paused by gateway configuration (index.paused = true). Cached search and manual refresh remain available.";
    default:
      return "The gateway does not report its scheduling policy. These controls only save this server's preference; upgrade the gateway to verify automatic scheduling.";
  }
}

export function autoRefreshErrorMessage(enabled: boolean): string {
  if (enabled) return "Unable to enable automatic index refresh.";
  return "Unable to disable automatic index refresh.";
}

export function nextSearchIndex(
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

export function highlightedRanges(text: string, query: string): TextRange[] {
  const terms = searchTerms(query);
  if (terms.length === 0) return [];
  return mergeTextRanges(findTextRanges(text, terms));
}

export function searchMatchMode(query: string): "prefix" | "contains" {
  return query.length < 3 ? "prefix" : "contains";
}
