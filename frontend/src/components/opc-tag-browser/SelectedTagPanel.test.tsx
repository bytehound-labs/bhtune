import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SelectedTagPanel } from "./SelectedTagPanel";

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

describe("SelectedTagPanel", () => {
  it("copies the exact selected ItemID", async () => {
    const itemId = "Unit1.LIC101!PV";
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(
      <SelectedTagPanel
        selectedTag={itemId}
        busy={false}
        selectionCheckPending={false}
        selectionReadError={null}
        testConnection={{
          isPending: false,
          isSuccess: false,
          isError: false,
          data: undefined,
          error: undefined,
        }}
        onRead={vi.fn()}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
      />,
    );

    expect(screen.getByText(itemId, { exact: true })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Copy ItemID" }));

    expect((await screen.findByRole("status")).textContent).toBe(
      "ItemID copied to the clipboard.",
    );
    expect(writeText).toHaveBeenCalledWith(itemId);
  });
});
