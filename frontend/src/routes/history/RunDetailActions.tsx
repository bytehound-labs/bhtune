import { Link } from "react-router";
import { runExportUrl } from "../../api/runs";
import type { StartRunRequest } from "../../api/runs";
import { Button, PageHeading } from "../../components/ui";

interface RunDetailActionsProps {
  readonly id: string | undefined;
  readonly runId: number;
  readonly isRunning: boolean;
  readonly hasSamples: boolean;
  readonly originalRequest?: StartRunRequest | null;
  readonly cancelPending: boolean;
  readonly deletePending: boolean;
  readonly duplicateTitle?: string;
  readonly onCancel: () => void;
  readonly onDelete: () => void;
  readonly onDuplicate: () => void;
}

export function RunDetailActions({
  id,
  runId,
  isRunning,
  hasSamples,
  originalRequest,
  cancelPending,
  deletePending,
  duplicateTitle,
  onCancel,
  onDelete,
  onDuplicate,
}: RunDetailActionsProps) {
  const canDuplicate =
    originalRequest !== null && originalRequest !== undefined;

  return (
    <PageHeading
      title={`Tune #${id ?? ""}`}
      actions={
        <>
          {isRunning && (
            <Button
              variant="danger"
              disabled={cancelPending}
              onClick={onCancel}
            >
              {cancelPending ? "Cancelling…" : "Cancel tune"}
            </Button>
          )}
          {!isRunning && hasSamples && (
            <>
              <a href={runExportUrl(runId, "csv")} download>
                <Button>Export CSV</Button>
              </a>
              <a href={runExportUrl(runId, "json")} download>
                <Button>Export JSON</Button>
              </a>
            </>
          )}
          {!isRunning && (
            <Button
              variant="danger"
              disabled={deletePending}
              onClick={onDelete}
            >
              {deletePending ? "Deleting…" : "Delete tune"}
            </Button>
          )}
          <Button
            disabled={!canDuplicate}
            title={duplicateTitle}
            onClick={onDuplicate}
          >
            Duplicate this run
          </Button>
          <Link to="/runs">
            <Button>Back to tune history</Button>
          </Link>
        </>
      }
    />
  );
}
