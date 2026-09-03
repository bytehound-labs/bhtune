import { useCallback, useEffect, useState } from "react";
import type { NewRunDraft } from "./runs";

const STORAGE_KEY = "bhtune.demo.new-run-draft";
const MAX_AGE_MS = 24 * 60 * 60 * 1000;

type StoredDraft = {
  readonly saved_at: number;
  readonly draft: NewRunDraft;
};

function remove() {
  window.localStorage.removeItem(STORAGE_KEY);
}

function load(): StoredDraft | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const stored = JSON.parse(raw) as Partial<StoredDraft>;
    const age =
      typeof stored.saved_at === "number"
        ? Date.now() - stored.saved_at
        : Number.NaN;
    if (
      !Number.isFinite(age) ||
      age < 0 ||
      age >= MAX_AGE_MS ||
      typeof stored.draft !== "object" ||
      stored.draft === null
    ) {
      remove();
      return null;
    }
    return stored as StoredDraft;
  } catch {
    remove();
    return null;
  }
}

/** Keeps the public Demo form private to this browser and expires it after 24 hours. */
export function useDemoDraft(enabled: boolean) {
  const [stored, setStored] = useState<StoredDraft | null>(() =>
    enabled ? load() : null,
  );

  useEffect(() => {
    if (!enabled || !stored) return;
    const remaining = MAX_AGE_MS - (Date.now() - stored.saved_at);
    const timer = window.setTimeout(
      () => {
        remove();
        setStored(null);
      },
      Math.max(0, remaining),
    );
    return () => window.clearTimeout(timer);
  }, [enabled, stored]);

  const save = useCallback(
    (next: NewRunDraft, savedAt = Date.now()): boolean => {
      try {
        const storedDraft = {
          saved_at: savedAt,
          draft: next,
        } satisfies StoredDraft;
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify(storedDraft));
        setStored(storedDraft);
        return true;
      } catch {
        // The form remains usable when browser storage is unavailable.
        return false;
      }
    },
    [],
  );

  return {
    draft: enabled ? (stored?.draft ?? null) : null,
    savedAt: enabled ? (stored?.saved_at ?? null) : null,
    save,
  };
}
