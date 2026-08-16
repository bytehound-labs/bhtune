import { QueryClient } from "@tanstack/react-query";
import { ApiError } from "./api/errors";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // A 4xx response (not found, bad request, conflict, ...) is a permanent failure --
      // retrying it can never succeed, and the library's default 3-retry/exponential-backoff
      // policy just stalls the UI in a loading state for several seconds before the error
      // banner finally appears (see `history-explorer-ui`'s delete-run E2E test, which is
      // what surfaced this against a genuine 404). Keep the default retry-3 behavior for
      // everything else (network failures, 5xx), where a retry can plausibly help.
      retry: (failureCount, error) =>
        error instanceof ApiError && error.status >= 400 && error.status < 500
          ? false
          : failureCount < 3,
    },
  },
});
