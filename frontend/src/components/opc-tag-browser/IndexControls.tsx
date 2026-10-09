import type { OpcSearchIndexStatusResponse } from "../../api/opc";
import { formatExactTime, formatTimeUntil } from "../../lib/time";
import { Button, LoadingStatus } from "../ui";
import { autoRefreshPolicyMessage } from "./searchModel";

type IndexControlsProps = Readonly<{
  opcServer: string;
  indexStatus: OpcSearchIndexStatusResponse | undefined;
  indexStateLabel: string | null;
  indexError: string | null;
  onIndexErrorShown: (element: HTMLOutputElement | null) => void;
  indexSearchAvailable: boolean;
  indexUnavailableMessage: string;
  refreshPending: boolean;
  controlPending: boolean;
  deletePending: boolean;
  onRefresh: () => void;
  onCancel: () => void;
  onDelete: () => void;
}>;

export function IndexControls({
  opcServer,
  indexStatus,
  indexStateLabel,
  indexError,
  onIndexErrorShown,
  indexSearchAvailable,
  indexUnavailableMessage: unavailableMessage,
  refreshPending,
  controlPending,
  deletePending,
  onRefresh,
  onCancel,
  onDelete,
}: IndexControlsProps) {
  const canCancelBuild =
    indexStatus?.state === "partial" || indexStatus?.state === "refreshing";
  const canDelete =
    indexStatus &&
    (indexStatus.active_generation > 0 || indexStatus.state === "failed");
  const deleteDisabled =
    deletePending ||
    refreshPending ||
    indexStatus?.state === "partial" ||
    indexStatus?.state === "refreshing";
  const policy = indexStatus?.scheduler.auto_refresh_policy;
  const policyBlocked = policy === "disabled" || policy === "paused";
  const policyMessage = autoRefreshPolicyMessage(policy);
  const nextRefreshAt =
    policy === "allowed" ? indexStatus?.scheduler.next_refresh_at : null;
  const autoRefreshLabel = policyBlocked
    ? "blocked by gateway"
    : !policy
      ? "policy unavailable"
      : nextRefreshAt
        ? "enabled"
        : "not scheduled";

  return (
    <div className="mb-3 space-y-2">
      <div className="flex gap-2">
        <Button
          type="button"
          disabled={
            refreshPending ||
            !opcServer ||
            indexStatus?.state === "partial" ||
            indexStatus?.state === "refreshing" ||
            indexStatus?.state === "deleting"
          }
          loading={refreshPending}
          onClick={onRefresh}
        >
          Refresh Index
        </Button>
        {canCancelBuild && (
          <Button
            type="button"
            loading={controlPending}
            disabled={controlPending}
            onClick={onCancel}
          >
            Cancel Indexing
          </Button>
        )}
        {indexStatus?.state === "deleting" && (
          <LoadingStatus
            message="Deleting the tag index… browse and direct reads remain available."
            size="sm"
            className="text-xs text-amber-300"
          />
        )}
        {canDelete && (
          <Button
            type="button"
            variant="danger"
            loading={deletePending}
            disabled={deleteDisabled}
            onClick={onDelete}
          >
            Delete Index
          </Button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-500">
        {indexStateLabel && (
          <span className="text-slate-400">
            Index: {indexStateLabel.toLocaleLowerCase()}
          </span>
        )}
        {indexStatus?.progress && (
          <span>
            {indexStatus.progress.entries_seen.toLocaleString()} entries{" · "}
            {indexStatus.progress.items_per_second.toFixed(0)} items/s
          </span>
        )}
        {nextRefreshAt && (
          <span title={formatExactTime(nextRefreshAt) ?? undefined}>
            Next refresh: {formatTimeUntil(nextRefreshAt)}
          </span>
        )}
        {indexStatus && indexStatus.active_generation > 0 && (
          <span
            title={
              policy === "allowed" && !nextRefreshAt
                ? "The gateway has not reported a scheduled refresh."
                : undefined
            }
          >
            Auto-refresh: {autoRefreshLabel}
          </span>
        )}
      </div>
      {indexStatus && indexStatus.active_generation > 0 && policyMessage && (
        <p id="opc-auto-refresh-policy" className="text-xs text-slate-400">
          {policyMessage}
        </p>
      )}
      {indexError && (
        <output ref={onIndexErrorShown} className="block text-xs text-red-300">
          Index error: {indexError}
        </output>
      )}
      {!indexSearchAvailable && (
        <output className="block text-xs text-slate-400">
          {unavailableMessage}
        </output>
      )}
    </div>
  );
}
