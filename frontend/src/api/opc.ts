import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { apiClient } from "./client";
import { ApiError, apiErrorMessage, toApiError } from "./errors";
import type { components } from "./schema";

export type OpcTagNodeResponse = components["schemas"]["OpcBrowseNodeResponse"];
export type OpcBrowseResponse = components["schemas"]["OpcBrowseResponse"];
export type OpcReadResponse = components["schemas"]["OpcReadResponse"];
export type OpcServersResponse = components["schemas"]["OpcServersResponse"];
export type OpcSearchIndexStatusResponse =
  components["schemas"]["OpcSearchIndexStatusResponse"];
export type OpcIndexedSearchMatchResponse =
  components["schemas"]["OpcIndexedSearchMatchResponse"];
export type OpcSearchIndexResponse =
  components["schemas"]["OpcSearchIndexResponse"];
export type OpcSearchMatchMode = "exact" | "prefix" | "contains";

export interface OpcBrowseBreadcrumbResponse {
  node_key: string;
  display_name: string;
}

export interface OpcSearchMatchResponse {
  node: OpcTagNodeResponse;
  breadcrumbs: OpcBrowseBreadcrumbResponse[];
}

export interface OpcSearchResponse {
  matches: OpcSearchMatchResponse[];
  complete: boolean;
  cancelled: boolean;
  truncated: boolean;
  warning: string | null;
}

export interface OpcSearchProgress {
  visited_nodes: number;
  matches: number;
  partial: boolean;
}

export interface OpcSearchCompleted {
  complete: boolean;
  cancelled: boolean;
  truncated: boolean;
  warning: string | null;
}

export type OpcSearchEvent =
  | { type: "match"; match: OpcSearchMatchResponse }
  | { type: "progress"; progress: OpcSearchProgress }
  | { type: "completed"; completed: OpcSearchCompleted };

export interface OpcBrowsePageRequest {
  sessionId?: string;
  parentNodeKey?: string;
  pageToken?: string;
  pageSize?: number;
  refresh?: boolean;
}

/**
 * `GET /api/opc/servers` -- lists every OPC DA server registered on the bridge gateway's own
 * host, powering the New tune form's "Browse servers" button (`ui-opc-browser`). `enabled`
 * gates the initial fetch: opening the form must not itself make a live network call to a
 * gateway that may not exist yet (or isn't even configured -- a fresh install may still be
 * using the simulator driver) -- the caller flips it on only once the engineer explicitly
 * asks to discover servers, and calls `refetch()` for a subsequent click.
 */
export function useOpcServers(bridgeHost: string, enabled: boolean) {
  return useQuery({
    queryKey: ["opc", "servers", bridgeHost],
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET(
        "/api/opc/servers",
        { params: { query: { bridge_host: bridgeHost || undefined } } },
      );
      if (error) throw toApiError(error, response);
      return data as OpcServersResponse;
    },
    enabled,
    retry: false,
  });
}

/**
 * Fetches one bounded page from `GET /api/opc/browse`. The gateway owns opaque browse
 * session, node, and page-token values; callers only round-trip them, never reconstruct
 * paths from display text or OPC punctuation.
 */
export function useOpcBrowseFetcher(bridgeHost: string, opcServer: string) {
  const queryClient = useQueryClient();
  const fetchPage = useCallback(
    (request: OpcBrowsePageRequest = {}) =>
      queryClient.fetchQuery({
        queryKey: ["opc", "browse", bridgeHost, opcServer, request],
        queryFn: async ({ signal }) => {
          const { data, error, response } = await apiClient.GET(
            "/api/opc/browse",
            {
              params: {
                query: {
                  bridge_host: bridgeHost || undefined,
                  opc_server: opcServer || undefined,
                  session_id: request.sessionId,
                  parent_node_key: request.parentNodeKey,
                  page_token: request.pageToken,
                  page_size: request.pageSize,
                  refresh: request.refresh || undefined,
                },
                signal,
              },
            },
          );
          if (error) throw toApiError(error, response);
          return data;
        },
        staleTime: 30_000,
      }),
    [bridgeHost, opcServer, queryClient],
  );
  const clearCache = useCallback(() => {
    const cacheKeyPrefix = ["opc", "browse", bridgeHost, opcServer] as const;
    void queryClient.cancelQueries({ queryKey: cacheKeyPrefix });
    queryClient.removeQueries({ queryKey: cacheKeyPrefix });
  }, [bridgeHost, opcServer, queryClient]);
  return { fetchPage, clearCache };
}

/** `DELETE /api/opc/browse/sessions/{session_id}` -- releases an open gateway browse session. */
export function useCloseOpcBrowseSession() {
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      sessionId: string;
    }) => {
      const { error, response } = await apiClient.DELETE(
        "/api/opc/browse/sessions/{session_id}",
        {
          params: {
            path: { session_id: params.sessionId },
            query: {
              bridge_host: params.bridgeHost || undefined,
              opc_server: params.opcServer || undefined,
            },
          },
        },
      );
      if (error) throw toApiError(error, response);
    },
  });
}

function searchUrl(params: {
  bridgeHost: string;
  opcServer: string;
  query: string;
  matchMode: OpcSearchMatchMode;
  sessionId?: string;
  scopeNodeKey?: string;
  maxResults?: number;
  includeBranches?: boolean;
  refresh?: boolean;
}) {
  const searchParams = new URLSearchParams();
  if (params.bridgeHost) searchParams.set("bridge_host", params.bridgeHost);
  if (params.opcServer) searchParams.set("opc_server", params.opcServer);
  searchParams.set("query", params.query);
  searchParams.set("match_mode", params.matchMode);
  if (params.sessionId) searchParams.set("session_id", params.sessionId);
  if (params.scopeNodeKey)
    searchParams.set("scope_node_key", params.scopeNodeKey);
  if (params.maxResults !== undefined)
    searchParams.set("max_results", String(params.maxResults));
  if (params.includeBranches !== undefined)
    searchParams.set("include_branches", String(params.includeBranches));
  if (params.refresh !== undefined)
    searchParams.set("refresh", String(params.refresh));
  return `/api/opc/search?${searchParams.toString()}`;
}

function parseJsonSearchMatch(data: string): OpcSearchMatchResponse | null {
  const parsed = JSON.parse(data) as unknown;
  if (
    typeof parsed === "object" &&
    parsed !== null &&
    "node" in parsed &&
    "breadcrumbs" in parsed
  ) {
    const match = parsed as OpcSearchMatchResponse;
    if (match.node.kind === ("branchanditem" as typeof match.node.kind)) {
      return {
        ...match,
        node: { ...match.node, kind: "branch_and_item" },
      };
    }
    return match;
  }
  return null;
}

function parseSearchEvent(
  block: string,
  response: Response,
): OpcSearchEvent | null {
  let event = "message";
  const dataLines: string[] = [];
  for (const line of block.split(/\r?\n/)) {
    if (line.startsWith("event:")) {
      event = line.slice("event:".length).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice("data:".length).trimStart());
    }
  }
  if (dataLines.length === 0) return null;
  const data = dataLines.join("\n");

  if (event === "match") {
    const match = parseJsonSearchMatch(data);
    return match ? { type: "match", match } : null;
  }
  if (event === "progress") {
    const parsed = JSON.parse(data) as Partial<OpcSearchProgress>;
    return {
      type: "progress",
      progress: {
        visited_nodes: parsed.visited_nodes ?? 0,
        matches: parsed.matches ?? 0,
        partial: parsed.partial ?? false,
      },
    };
  }
  if (event === "completed") {
    const parsed = JSON.parse(data) as Partial<OpcSearchCompleted>;
    return {
      type: "completed",
      completed: {
        complete: parsed.complete ?? true,
        cancelled: parsed.cancelled ?? false,
        truncated: parsed.truncated ?? false,
        warning: parsed.warning ?? null,
      },
    };
  }
  if (event === "error") {
    throw new ApiError(apiErrorMessage(JSON.parse(data)), response.status);
  }
  return null;
}

function consumeSearchEvent(
  result: OpcSearchResponse,
  event: OpcSearchEvent,
  onEvent: ((event: OpcSearchEvent) => void) | undefined,
) {
  onEvent?.(event);
  if (event.type === "match") {
    result.matches.push(event.match);
  } else if (event.type === "completed") {
    result.complete = event.completed.complete;
    result.cancelled = event.completed.cancelled;
    result.truncated = event.completed.truncated;
    result.warning = event.completed.warning;
  }
}

async function parseOpcSearchStream(
  response: Response,
  onEvent?: (event: OpcSearchEvent) => void,
): Promise<OpcSearchResponse> {
  const result: OpcSearchResponse = {
    matches: [],
    complete: true,
    cancelled: false,
    truncated: false,
    warning: null,
  };
  const reader = response.body?.getReader();
  if (!reader) {
    const eventText = await response.text();
    for (const block of eventText.split(/\r?\n\r?\n/)) {
      if (!block.trim()) continue;
      const event = parseSearchEvent(block, response);
      if (event) consumeSearchEvent(result, event, onEvent);
    }
    return result;
  }

  const decoder = new TextDecoder();
  let buffer = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      const blocks = buffer.split(/\r?\n\r?\n/);
      buffer = blocks.pop() ?? "";
      for (const block of blocks) {
        if (!block.trim()) continue;
        const event = parseSearchEvent(block, response);
        if (event) consumeSearchEvent(result, event, onEvent);
      }
      if (done) break;
    }
    if (buffer.trim()) {
      const event = parseSearchEvent(buffer, response);
      if (event) consumeSearchEvent(result, event, onEvent);
    }
  } finally {
    reader.releaseLock();
  }
  return result;
}

/** `GET /api/opc/search` -- finds selectable ItemIDs without inferring paths from punctuation. */
export function useOpcSearch() {
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      query: string;
      matchMode: OpcSearchMatchMode;
      sessionId?: string;
      scopeNodeKey?: string;
      maxResults?: number;
      includeBranches?: boolean;
      refresh?: boolean;
      signal?: AbortSignal;
      onEvent?: (event: OpcSearchEvent) => void;
    }) => {
      const response = await fetch(searchUrl(params), {
        signal: params.signal,
      });
      if (!response.ok) {
        const error = (await response.json().catch(() => null)) as unknown;
        throw new ApiError(apiErrorMessage(error), response.status);
      }
      if (response.headers.get("content-type")?.includes("application/json")) {
        return (await response.json()) as OpcSearchResponse;
      }
      return parseOpcSearchStream(response, params.onEvent);
    },
  });
}

/** `GET /api/opc/search-index/status` -- returns persistent namespace-index readiness. */
export function useOpcSearchIndexStatus(
  bridgeHost: string,
  opcServer: string,
  enabled: boolean,
) {
  return useQuery({
    queryKey: ["opc", "search-index", "status", bridgeHost, opcServer],
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET(
        "/api/opc/search-index/status",
        {
          params: {
            query: {
              bridge_host: bridgeHost || undefined,
              opc_server: opcServer || undefined,
            },
          },
        },
      );
      if (error) throw toApiError(error, response);
      return data as OpcSearchIndexStatusResponse;
    },
    enabled,
    retry: false,
    staleTime: 5_000,
  });
}

/** `GET /api/opc/search-index/search` -- fast unary fzf-style namespace search. */
export function useOpcIndexedSearch() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      query: string;
      matchMode: OpcSearchMatchMode;
      maxResults?: number;
      signal?: AbortSignal;
    }) => {
      const { data, error, response } = await apiClient.GET(
        "/api/opc/search-index/search",
        {
          params: {
            query: {
              bridge_host: params.bridgeHost || undefined,
              opc_server: params.opcServer || undefined,
              query: params.query,
              match_mode: params.matchMode,
              max_results: params.maxResults,
            },
            signal: params.signal,
          },
        },
      );
      if (error) throw toApiError(error, response);
      return data as OpcSearchIndexResponse;
    },
    onSuccess: (data, variables) => {
      queryClient.setQueryData(
        [
          "opc",
          "search-index",
          "status",
          variables.bridgeHost,
          variables.opcServer,
        ],
        data.status,
      );
    },
  });
}

/** `POST /api/opc/search-index/refresh` -- starts or coalesces an index refresh. */
export function useRefreshOpcSearchIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      force?: boolean;
    }) => {
      const { data, error, response } = await apiClient.POST(
        "/api/opc/search-index/refresh",
        {
          params: {
            query: {
              bridge_host: params.bridgeHost || undefined,
              opc_server: params.opcServer || undefined,
              force: params.force,
            },
          },
        },
      );
      if (error) throw toApiError(error, response);
      return data as OpcSearchIndexStatusResponse;
    },
    onSuccess: (data, variables) => {
      queryClient.setQueryData(
        [
          "opc",
          "search-index",
          "status",
          variables.bridgeHost,
          variables.opcServer,
        ],
        data,
      );
    },
  });
}

/** `POST /api/opc/search-index/control` -- controls a running index build. */
export function useControlOpcSearchIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      action: "pause" | "resume" | "cancel";
    }) => {
      const { data, error, response } = await apiClient.POST(
        "/api/opc/search-index/control",
        {
          params: {
            query: {
              bridge_host: params.bridgeHost || undefined,
              opc_server: params.opcServer || undefined,
              action: params.action,
            },
          },
        },
      );
      if (error) throw toApiError(error, response);
      return data as OpcSearchIndexStatusResponse;
    },
    onSuccess: (data, variables) => {
      queryClient.setQueryData(
        [
          "opc",
          "search-index",
          "status",
          variables.bridgeHost,
          variables.opcServer,
        ],
        data,
      );
    },
  });
}

/**
 * `GET /api/opc/read` -- backs the "Read selected tag" diagnostic action and the final
 * selection quality check in the OPC tag-tree browser. Modeled as a mutation even though it's
 * a read-only `GET`: it's fired on demand against a *different* tag on every click rather than
 * representing one cacheable resource, exactly like `useWriteRun`/`useRevertRun` (`api/runs.ts`)
 * model their own on-demand server calls.
 */
export function useTestOpcConnection() {
  return useMutation({
    mutationFn: async (params: {
      bridgeHost: string;
      opcServer: string;
      tag: string;
    }) => {
      const { data, error, response } = await apiClient.GET("/api/opc/read", {
        params: {
          query: {
            bridge_host: params.bridgeHost || undefined,
            opc_server: params.opcServer || undefined,
            tag: params.tag,
          },
        },
      });
      if (error) throw toApiError(error, response);
      return data;
    },
  });
}
