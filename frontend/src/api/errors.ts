/**
 * Every fallible route in `bhtune-server` returns a JSON `{"error": "<message>"}` body on
 * non-2xx responses (`bhtune_server::error::ErrorBody`), typed in the generated client as
 * `components["schemas"]["ErrorBody"]`. This narrows an `openapi-fetch` `error` value (whose
 * static type varies per-operation, since not every status code shares the same schema) down
 * to a displayable string, falling back to a generic message for the rare response that truly
 * carries no body (e.g. a network failure `openapi-fetch` surfaces before any response
 * exists).
 */
export function apiErrorMessage(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "error" in error &&
    typeof (error as { error: unknown }).error === "string"
  ) {
    return (error as { error: string }).error;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return "request failed";
}

/**
 * An API failure that remembers its HTTP status code, so callers (namely
 * `queryClient`'s default `retry`) can tell a permanent client error (404, 400, 409, ...)
 * apart from a transient one (a 5xx, or a network failure) without re-parsing the message.
 */
export class ApiError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/**
 * Builds the `ApiError` a `queryFn`/`mutationFn` should throw for a failed `openapi-fetch`
 * call: `apiErrorMessage(error)` for the text, `response.status` for the code that lets
 * `queryClient`'s default `retry` skip retrying permanent 4xx failures (see `queryClient.ts`)
 * instead of stalling the UI in a loading state through several pointless retries.
 */
export function toApiError(error: unknown, response: Response): ApiError {
  return new ApiError(apiErrorMessage(error), response.status);
}
