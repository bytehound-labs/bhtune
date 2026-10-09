import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Button, ErrorBanner, InlineStatus } from "./ui";

afterEach(cleanup);

describe("Button", () => {
  it("preserves a disabled action's accessible explanation", () => {
    render(
      <>
        <Button disabled aria-describedby="refresh-policy">
          Enable auto-refresh
        </Button>
        <p id="refresh-policy">
          Automatic refresh is blocked by gateway configuration.
        </p>
      </>,
    );

    const button = screen.getByRole("button", {
      name: "Enable auto-refresh",
      description: "Automatic refresh is blocked by gateway configuration.",
    });
    expect(button.hasAttribute("disabled")).toBe(true);
    expect(button.getAttribute("aria-describedby")).toBe("refresh-policy");
  });
});

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
