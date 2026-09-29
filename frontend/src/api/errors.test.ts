import { describe, expect, it } from "vitest";
import {
  ApiError,
  apiErrorMessage,
  toApiError,
  userFacingErrorMessage,
} from "./errors";

describe("apiErrorMessage", () => {
  it("returns a string from the API error body", () => {
    expect(apiErrorMessage({ error: "Template not found." })).toBe(
      "Template not found.",
    );
  });

  it("uses an Error message when there is no API error body", () => {
    expect(apiErrorMessage(new Error("network disconnected"))).toBe(
      "network disconnected",
    );
  });

  it("uses the generic message for an unrecognized error", () => {
    expect(apiErrorMessage({ error: 404 })).toBe("request failed");
    expect(apiErrorMessage(null)).toBe("request failed");
  });
});

describe("toApiError", () => {
  it("preserves the HTTP status and a readable message", () => {
    const error = toApiError(
      { error: "Template not found." },
      new Response(null, { status: 404 }),
    );

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      name: "ApiError",
      message: "Template not found.",
      status: 404,
    });
  });
});

describe("userFacingErrorMessage", () => {
  it("shows the Demo quota explanation for 429 responses", () => {
    expect(
      userFacingErrorMessage(
        new ApiError("private quota detail", 429),
        "Fallback",
        true,
      ),
    ).toBe(
      "Demo usage limit reached. Wait a few minutes before retrying; repeated attempts will not reset the limit.",
    );
  });

  it("shows the Demo service explanation for 503 responses", () => {
    expect(
      userFacingErrorMessage(
        new ApiError("private service detail", 503),
        "Fallback",
        true,
      ),
    ).toBe(
      "The demo service is temporarily unavailable. Wait a moment and retry; if it continues, come back later.",
    );
  });

  it("explains an origin mismatch in Demo mode", () => {
    expect(
      userFacingErrorMessage(
        new ApiError("private origin detail", 403),
        "Fallback",
        true,
      ),
    ).toBe(
      "This Demo page is not using the configured browser URL. Open the Demo through its configured browser origin and try again.",
    );
  });

  it("uses a generic message for server errors", () => {
    expect(
      userFacingErrorMessage(new ApiError("private detail", 500), "Fallback"),
    ).toBe("The server could not complete the request. Try again.");
  });

  it("explains a connection failure with status zero", () => {
    expect(
      userFacingErrorMessage(new ApiError("private detail", 0), "Fallback"),
    ).toBe(
      "Unable to reach the BHTune server. Check the connection and try again.",
    );
  });

  it("uses the supplied fallback for other client errors and non-API failures", () => {
    expect(
      userFacingErrorMessage(new ApiError("private detail", 404), "Fallback"),
    ).toBe("Fallback");
    expect(
      userFacingErrorMessage(new Error("private detail"), "Fallback"),
    ).toBe("Fallback");
  });
});
