import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import type { PropsWithChildren } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { apiClient } from "./client";
import { useDeleteTemplate } from "./templates";

vi.mock("./client", () => ({
  apiClient: {
    DELETE: vi.fn(),
  },
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function wrapperFor(queryClient: QueryClient) {
  return function Wrapper({ children }: PropsWithChildren) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

describe("useDeleteTemplate", () => {
  it("refreshes only the template list and removes the deleted detail query", async () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });
    const name = "Yokogawa CentumVP";
    const listKey = ["templates"] as const;
    const deletedDetailKey = ["templates", name] as const;
    const otherDetailKey = ["templates", "Honeywell Experion"] as const;
    const listQuery = vi.fn().mockResolvedValue([]);
    const deletedDetailQuery = vi.fn().mockResolvedValue({ name });
    const otherDetailQuery = vi
      .fn()
      .mockResolvedValue({ name: "Honeywell Experion" });

    queryClient.setQueryData(listKey, []);
    queryClient.setQueryData(deletedDetailKey, { name });
    queryClient.setQueryData(otherDetailKey, { name: "Honeywell Experion" });

    const wrapper = wrapperFor(queryClient);
    renderHook(
      () =>
        useQuery({
          queryKey: listKey,
          queryFn: listQuery,
          staleTime: Infinity,
        }),
      { wrapper },
    );
    renderHook(
      () =>
        useQuery({
          queryKey: deletedDetailKey,
          queryFn: deletedDetailQuery,
          staleTime: Infinity,
        }),
      { wrapper },
    );
    renderHook(
      () =>
        useQuery({
          queryKey: otherDetailKey,
          queryFn: otherDetailQuery,
          staleTime: Infinity,
        }),
      { wrapper },
    );
    const mutation = renderHook(() => useDeleteTemplate(), { wrapper });
    vi.mocked(apiClient.DELETE).mockResolvedValue({
      data: undefined,
      response: new Response(null, { status: 204 }),
    });

    await act(async () => {
      await mutation.result.current.mutateAsync(name);
    });

    await waitFor(() => expect(listQuery).toHaveBeenCalledOnce());
    expect(queryClient.getQueryState(deletedDetailKey)).toBeUndefined();
    expect(queryClient.getQueryState(otherDetailKey)?.isInvalidated).toBe(
      false,
    );
    expect(deletedDetailQuery).not.toHaveBeenCalled();
    expect(otherDetailQuery).not.toHaveBeenCalled();

    queryClient.clear();
  });
});
