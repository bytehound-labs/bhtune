import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ErrorBanner } from "./ui";

afterEach(cleanup);

describe("ErrorBanner", () => {
  it("renders its message as an accessible alert", () => {
    render(<ErrorBanner message="The request could not be completed." />);

    expect(screen.getByRole("alert").textContent).toBe(
      "The request could not be completed.",
    );
  });
});
