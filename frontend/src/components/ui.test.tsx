import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ErrorBanner, InlineStatus } from "./ui";

afterEach(cleanup);

describe("ErrorBanner", () => {
  it("renders its message as an accessible alert", () => {
    render(<ErrorBanner message="The request could not be completed." />);

    expect(screen.getByRole("alert").textContent).toBe(
      "The request could not be completed.",
    );
  });
});

describe("InlineStatus", () => {
  it("announces success as a polite status", () => {
    render(<InlineStatus message="Configuration saved successfully." />);

    expect(screen.getByRole("status").textContent).toBe(
      "Configuration saved successfully.",
    );
    expect(screen.getByRole("status").getAttribute("aria-live")).toBe("polite");
  });

  it("announces failures as alerts", () => {
    render(<InlineStatus message="Unable to copy the ItemID." tone="error" />);

    expect(screen.getByRole("alert").textContent).toBe(
      "Unable to copy the ItemID.",
    );
    expect(screen.getByRole("alert").getAttribute("aria-live")).toBe(
      "assertive",
    );
  });
});
