import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CopyButton } from "./CopyButton";

const originalClipboard = Object.getOwnPropertyDescriptor(
  navigator,
  "clipboard",
);

afterEach(() => {
  cleanup();
  if (originalClipboard) {
    Object.defineProperty(navigator, "clipboard", originalClipboard);
  } else {
    Reflect.deleteProperty(navigator, "clipboard");
  }
  vi.restoreAllMocks();
});

describe("CopyButton", () => {
  it("reports success only after the clipboard write resolves", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<CopyButton value="Area01.FIC101.PV" label="ItemID" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy ItemID" }));

    expect((await screen.findByRole("status")).textContent).toBe(
      "ItemID copied to the clipboard.",
    );
    expect(writeText).toHaveBeenCalledOnce();
    expect(writeText).toHaveBeenCalledWith("Area01.FIC101.PV");
  });

  it("reports clipboard rejection without claiming success", async () => {
    const writeText = vi
      .fn()
      .mockRejectedValue(new Error("Clipboard permission denied"));
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(<CopyButton value="4242" label="run ID" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy run ID" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Unable to copy the run ID. Copy it manually.",
    );
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("explains when clipboard access is unavailable", async () => {
    Reflect.deleteProperty(navigator, "clipboard");
    render(<CopyButton value="Area01.FIC101.PV" label="ItemID" />);

    fireEvent.click(screen.getByRole("button", { name: "Copy ItemID" }));

    expect((await screen.findByRole("alert")).textContent).toBe(
      "Clipboard access is unavailable. Copy the ItemID manually.",
    );
  });
});
