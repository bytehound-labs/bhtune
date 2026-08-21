import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiClient } from "./client";
import { toApiError } from "./errors";
import type { components } from "./schema";

export type OpcTagNodeResponse = components["schemas"]["OpcTagNodeResponse"];
export type OpcReadResponse = components["schemas"]["OpcReadResponse"];

/**
 * `GET /api/opc/servers` -- lists every OPC DA server registered on the bridge gateway's own
 * host, powering the New tune form's server-discovery button (`ui-opc-browser`). `enabled`
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
 * Fetches (and caches, through the query client) one level of `GET /api/opc/browse`'s tag
 * tree. Not a `useQuery` itself: the tag-tree browser modal expands an unbounded, user-driven
 * set of `path`s as branches are clicked open one at a time, which doesn't fit a single fixed
 * hook call the way a normal list/detail fetch does. Still routed through
 * `queryClient.fetchQuery` rather than a bare `apiClient` call, so re-expanding an
 * already-visited branch within the cache's `staleTime` is free and this stays consistent
 * with the rest of the app's "TanStack Query is the only data-fetching/caching layer"
 * convention (see `App.tsx`'s routing comment).
 */
export function useOpcBrowseFetcher(bridgeHost: string, opcServer: string) {
  const queryClient = useQueryClient();
  return (path: string) =>
    queryClient.fetchQuery({
      queryKey: ["opc", "browse", bridgeHost, opcServer, path],
      queryFn: async () => {
        const { data, error, response } = await apiClient.GET(
          "/api/opc/browse",
          {
            params: {
              query: {
                bridge_host: bridgeHost || undefined,
                opc_server: opcServer || undefined,
                path: path || undefined,
              },
            },
          },
        );
        if (error) throw toApiError(error, response);
        return data.nodes;
      },
      staleTime: 30_000,
    });
}

/**
 * `GET /api/opc/read` -- backs the "Test connection" button in the OPC tag-tree browser.
 * Modeled as a mutation even though it's a read-only `GET`: it's fired on demand against a
 * *different* tag on every click rather than representing one cacheable resource, exactly
 * like `useWriteRun`/`useRevertRun` (`api/runs.ts`) model their own on-demand server calls.
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
