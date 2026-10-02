import { describe, expect, it } from "vitest";
import {
  HISTORY_PAGE_SIZE,
  emptyHistoryUrlState,
  historyUrlSearchParams,
  parseHistoryUrlState,
} from "./historyUrlState";

describe("history URL state", () => {
  it("parses and serializes the visible filters and page offset", () => {
    const searchParams = new URLSearchParams(
      "process_type=flow&outcome=completed&driver=simulator&offset=50&source=link",
    );

    const parsed = parseHistoryUrlState(searchParams, false);

    expect(parsed).toEqual({
      state: {
        processType: "flow",
        outcome: "completed",
        driver: "simulator",
        offset: HISTORY_PAGE_SIZE,
      },
      invalidParameters: [],
    });
    expect(
      historyUrlSearchParams(searchParams, parsed.state, false).toString(),
    ).toBe(
      "source=link&process_type=flow&outcome=completed&driver=simulator&offset=50",
    );
  });

  it("resets malformed filters and page offsets while preserving unrelated query values", () => {
    const searchParams = new URLSearchParams(
      "process_type=unknown&outcome=running%2Cfailed&driver=opcda&offset=12&view=compact",
    );

    const parsed = parseHistoryUrlState(searchParams, false);

    expect(parsed.state).toEqual({
      ...emptyHistoryUrlState(),
      driver: "opcda",
    });
    expect(parsed.invalidParameters).toEqual([
      "process_type",
      "outcome",
      "offset",
    ]);
    expect(
      historyUrlSearchParams(searchParams, parsed.state, false).toString(),
    ).toBe("view=compact&driver=opcda");
  });

  it("removes Full-only filters from Demo URLs without changing valid pagination", () => {
    const searchParams = new URLSearchParams(
      "process_type=flow&driver=opcda&offset=50",
    );

    const parsed = parseHistoryUrlState(searchParams, true);

    expect(parsed.state).toEqual({
      ...emptyHistoryUrlState(),
      offset: HISTORY_PAGE_SIZE,
    });
    expect(parsed.invalidParameters).toEqual(["process_type", "driver"]);
    expect(
      historyUrlSearchParams(searchParams, parsed.state, true).toString(),
    ).toBe("offset=50");
  });

  it("resets repeated filter and offset parameters", () => {
    const parsed = parseHistoryUrlState(
      new URLSearchParams(
        "process_type=flow&process_type=level&offset=50&offset=100",
      ),
      false,
    );

    expect(parsed.state).toEqual(emptyHistoryUrlState());
    expect(parsed.invalidParameters).toEqual(["process_type", "offset"]);
  });
});
