import { useState } from "react";
import { InlineStatus } from "./ui";

type CopyFeedback = {
  readonly value: string;
  readonly tone: "success" | "error";
  readonly message: string;
};

export function CopyButton({
  value,
  label,
}: {
  readonly value: string;
  readonly label: string;
}) {
  const [copying, setCopying] = useState(false);
  const [feedback, setFeedback] = useState<CopyFeedback | null>(null);
  const currentFeedback = feedback?.value === value ? feedback : null;

  async function copyValue() {
    if (copying) return;
    setFeedback(null);

    const clipboard =
      typeof navigator === "undefined" ? undefined : navigator.clipboard;
    if (!clipboard || typeof clipboard.writeText !== "function") {
      setFeedback({
        value,
        tone: "error",
        message: `Clipboard access is unavailable. Copy the ${label} manually.`,
      });
      return;
    }

    setCopying(true);
    try {
      await clipboard.writeText(value);
      setFeedback({
        value,
        tone: "success",
        message: `${label} copied to the clipboard.`,
      });
    } catch {
      setFeedback({
        value,
        tone: "error",
        message: `Unable to copy the ${label}. Copy it manually.`,
      });
    } finally {
      setCopying(false);
    }
  }

  return (
    <span
      data-testid="copy-button-control"
      className="inline-flex max-w-full flex-wrap items-center gap-2"
    >
      <button
        type="button"
        aria-label={`Copy ${label}`}
        title={`Copy ${label}`}
        disabled={copying || value.length === 0}
        onClick={() => void copyValue()}
        className="inline-flex shrink-0 items-center rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-200 transition-colors hover:bg-slate-800 focus:outline-none focus:ring-2 focus:ring-slate-500 disabled:cursor-not-allowed disabled:opacity-60"
      >
        {copying ? "Copying…" : "Copy"}
      </button>
      {currentFeedback && (
        <InlineStatus
          message={currentFeedback.message}
          tone={currentFeedback.tone}
          className="px-2 py-1 text-xs"
        />
      )}
    </span>
  );
}
