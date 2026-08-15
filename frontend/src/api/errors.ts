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
