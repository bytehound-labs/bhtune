import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { apiClient } from "./client";
import { toApiError } from "./errors";
import type { components } from "./schema";

export type OpcTagNodeResponse = components["schemas"]["OpcBrowseNodeResponse"];
export type OpcBrowseResponse = components["schemas"]["OpcBrowseResponse"];
export type OpcReadResponse = components["schemas"]["OpcReadResponse"];
export type OpcSearchIndexStatusResponse =
  components["schemas"]["OpcSearchIndexStatusResponse"];
export type OpcIndexedSearchMatchResponse =
  components["schemas"]["OpcIndexedSearchMatchResponse"];
export type OpcSearchIndexResponse =
  components["schemas"]["OpcSearchIndexResponse"];
export type OpcSearchMatchMode = "exact" | "prefix" | "contains";

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
      return data;
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
        retry: false,
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
