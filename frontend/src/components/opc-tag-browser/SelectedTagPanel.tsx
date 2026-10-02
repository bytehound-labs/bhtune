import type { OpcReadResponse } from "../../api/opc";
import { userFacingErrorMessage } from "../../api/errors";
import {
  SAMPLE_QUALITY_LABELS,
  SAMPLE_QUALITY_TONE,
} from "../../lib/enumLabels";
import { CopyButton } from "../CopyButton";
import { Badge, Button, ErrorBanner } from "../ui";

type SelectedTagPanelProps = Readonly<{
  selectedTag: string | null;
  busy: boolean;
  selectionCheckPending: boolean;
  selectionReadError: string | null;
  testConnection: {
    isPending: boolean;
    isSuccess: boolean;
    isError: boolean;
    data: OpcReadResponse | undefined;
    error: unknown;
  };
  onRead: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}>;

export function SelectedTagPanel({
  selectedTag,
  busy,
  selectionCheckPending,
  selectionReadError,
  testConnection,
  onRead,
  onCancel,
  onConfirm,
}: SelectedTagPanelProps) {
  return (
    <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
      {!selectedTag && (
        <p className="text-sm text-slate-400">
          Select a tag to test its live value and quality.
        </p>
      )}
      {selectedTag && (
        <>
          <div className="flex max-w-full flex-wrap items-center gap-2">
            <p className="text-sm text-slate-200">
              Selected:{" "}
              <span className="break-all font-mono">{selectedTag}</span>
            </p>
            <CopyButton value={selectedTag} label="ItemID" />
          </div>
          <p className="mt-1 text-xs text-slate-500">
            Select tag applies the active template&apos;s process-variable
            suffix. Review or override the rest of the mapping in the collapsed
            section on the main tune form.
          </p>

          <div className="mt-3 flex items-center gap-2">
            <Button
              loading={selectionCheckPending || testConnection.isPending}
              disabled={busy}
              onClick={onRead}
            >
              Read selected tag
            </Button>
            {testConnection.isSuccess && testConnection.data && (
              <span className="text-xs text-slate-300">
                {testConnection.data.value}{" "}
                <Badge tone={SAMPLE_QUALITY_TONE[testConnection.data.quality]}>
                  {SAMPLE_QUALITY_LABELS[testConnection.data.quality]}
                </Badge>
              </span>
            )}
            {testConnection.isError && !selectionReadError && (
              <span className="text-xs text-red-400">
                {userFacingErrorMessage(
                  testConnection.error,
                  "Unable to read the selected tag.",
                )}
              </span>
            )}
            {selectionReadError && <ErrorBanner message={selectionReadError} />}
          </div>

          <div className="mt-3 flex justify-end gap-2">
            <Button onClick={onCancel}>Cancel</Button>
            <Button
              variant="primary"
              loading={selectionCheckPending}
              disabled={busy}
              onClick={onConfirm}
            >
              Select tag
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
