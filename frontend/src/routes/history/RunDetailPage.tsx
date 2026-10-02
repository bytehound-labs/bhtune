import { useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router";
import { userFacingErrorMessage } from "../../api/errors";
import {
  useCancelRun,
  useDeleteRunNotes,
  useDeleteRun,
  useRevertRun,
  useRun,
  useRunStream,
  useUpdateRunNotes,
  useWriteRun,
  type RunDetailResponse,
  type SampleResponse,
} from "../../api/runs";
import type { DuplicateRunState } from "../runs/NewRunPage";
import { composeTrendPoints } from "../../lib/trend";
import {
  type ValidRunResult,
  type RunWrite,
  writeEligibility,
  writeFailureMessage,
} from "./runDetailHelpers";
import { RunDetailActions } from "./RunDetailActions";
import { PidActionModal, type PidAction } from "./PidActionModal";
import {
  RunDetailContent,
  RunDetailErrors,
  type RunErrorItem,
} from "./RunDetailSections";
import { ConfirmModal, ErrorBanner, LoadingState } from "../../components/ui";
import { OUTCOME_LABELS, RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";
import type { AppCapabilities } from "../../api/capabilities";

const EMPTY_TREND_SAMPLES: readonly SampleResponse[] = [];

function initialNoteState(run: RunDetailResponse | undefined, runId: number) {
  const currentRun = run?.id === runId ? run : undefined;
  const initialNotes = currentRun?.notes;
  return {
    sourceRunId: currentRun ? runId : null,
    sourceNotes: initialNotes,
    notes: initialNotes ?? "",
    notesDirty: false,
  };
}

function describeRunStatus(run: RunDetailResponse | undefined): string {
  if (!run) return "";
  const announcement = `Tune ${OUTCOME_LABELS[run.outcome].toLowerCase()}.`;
  if (run.restore_status !== "incomplete") return announcement;
  return `${announcement} Loop restoration is incomplete; review the run details before continuing.`;
}

function canRevertLastWrite(
  eligible: boolean,
  lastWrite: RunWrite | undefined,
): boolean {
  return Boolean(eligible && lastWrite?.kind === "write" && lastWrite.success);
}

function pidActionErrorFor(
  action: PidAction | null,
  writeError: Error | null,
  revertError: Error | null,
): Error | null {
  if (!action) return null;
  if (action.kind === "write") return writeError;
  return revertError;
}

function duplicateRunTitle(
  run: RunDetailResponse | undefined,
  isSuccess: boolean,
): string | undefined {
  if (isSuccess && run && !run.original_request) {
    return "This tune's original settings weren't recorded and can't be duplicated.";
  }
  return undefined;
}

function shouldShowPidActionModal(
  hasRun: boolean,
  isDemo: boolean,
  canWrite: boolean,
  canRevert: boolean,
): boolean {
  if (!hasRun || isDemo) return false;
  return canWrite && canRevert;
}

function returnHistoryHref(state: unknown): string {
  if (!state || typeof state !== "object" || !("historyHref" in state)) {
    return "/runs";
  }
  const historyHref = state.historyHref;
  if (typeof historyHref !== "string") return "/runs";

  try {
    const parsed = new URL(historyHref, "https://bhtune.invalid");
    if (
      parsed.origin !== "https://bhtune.invalid" ||
      parsed.pathname !== "/runs" ||
      parsed.hash
    ) {
      return "/runs";
    }
    return `${parsed.pathname}${parsed.search}`;
  } catch {
    return "/runs";
  }
}

function RunStatusAnnouncement({
  run,
}: {
  readonly run: RunDetailResponse | undefined;
}) {
  if (!run) return null;
  return (
    <div
      className="sr-only"
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      {describeRunStatus(run)}
    </div>
  );
}

export function RunDetailPage({
  capabilities,
}: {
  readonly capabilities: AppCapabilities;
}) {
  const { id } = useParams<{ id: string }>();
  const runId = Number(id);
  const location = useLocation();
  const navigate = useNavigate();
  const historyHref = returnHistoryHref(location.state);
  const isDemo = capabilities.mode === "demo";
  const apiMode = isDemo ? "demo" : "full";
  const run = useRun(runId, true, apiMode);
  const cancelRun = useCancelRun(apiMode);
  const updateNotes = useUpdateRunNotes();
  const deleteNotes = useDeleteRunNotes();
  const deleteRun = useDeleteRun(apiMode);
  const writeRun = useWriteRun();
  const revertRun = useRevertRun();
  const isRunning = run.data?.outcome === "running";
  const hasSamples = (run.data?.samples.length ?? 0) > 0;
  const eligibility = run.data
    ? writeEligibility(run.data)
    : { eligible: false };
  const writes = run.data?.writes ?? [];
  const lastWrite = writes.at(-1);
  // Restore always targets the newest WriteKind::Write row server-side. Only offer it
  // while that row is still newest, so a superseded restore action cannot mislead.
  const canRevert = canRevertLastWrite(eligibility.eligible, lastWrite);
  const stream = useRunStream(
    runId,
    isRunning && capabilities.actions.stream_run,
    apiMode,
  );
  const initialReadings = stream.initialReadings ?? run.data?.initial_readings;
  // The live SSE feed replays every sample from tick 0. Once terminal, the REST payload is
  // the cheaper source because there is no stream left to keep open.
  const trendSamples = isRunning
    ? stream.samples
    : (run.data?.samples ?? EMPTY_TREND_SAMPLES);
  const trendPollIntervalMs = run.data?.effective_tuning?.poll_interval_ms;
  const trendPoints = useMemo(() => {
    if (!run.data) return [];

    return composeTrendPoints(
      trendSamples,
      initialReadings,
      run.data.started_at,
      run.data.completed_at,
      !isRunning && run.data.restore_status !== "incomplete",
    );
  }, [initialReadings, isRunning, run.data, trendSamples]);
  const [noteState, setNoteState] = useState(() =>
    initialNoteState(run.data, runId),
  );
  const { notes, notesDirty } = noteState;
  const setNotes = (value: string) => {
    setNoteState((previous) => ({ ...previous, notes: value }));
  };
  const setNotesDirty = (value: boolean) => {
    setNoteState((previous) => ({ ...previous, notesDirty: value }));
  };
  const [pidAction, setPidAction] = useState<PidAction | null>(null);
  const [pidActionAlert, setPidActionAlert] = useState<string | null>(null);
  const [deleteConfirmationOpen, setDeleteConfirmationOpen] = useState(false);
  const pidActionPending = writeRun.isPending || revertRun.isPending;
  const pidActionError = pidActionErrorFor(
    pidAction,
    writeRun.error,
    revertRun.error,
  );

  if (
    run.data?.id === runId &&
    (noteState.sourceRunId !== runId ||
      noteState.sourceNotes !== run.data.notes)
  ) {
    setNoteState({
      sourceRunId: runId,
      sourceNotes: run.data.notes,
      notes: run.data.notes ?? "",
      notesDirty: false,
    });
  }

  function saveNotes() {
    updateNotes.mutate(
      { id: runId, notes },
      {
        onSuccess: (data) => {
          setNotes(data.notes ?? "");
          setNotesDirty(false);
        },
      },
    );
  }

  function clearNotes() {
    deleteNotes.mutate(runId, {
      onSuccess: (data) => {
        setNotes(data.notes ?? "");
        setNotesDirty(false);
      },
    });
  }

  function handleDelete() {
    deleteRun.reset();
    setDeleteConfirmationOpen(true);
  }

  function cancelDelete() {
    if (deleteRun.isPending) return;
    deleteRun.reset();
    setDeleteConfirmationOpen(false);
  }

  function confirmDelete() {
    if (deleteRun.isPending) return;
    deleteRun.mutate(runId, {
      onSuccess: () => {
        setDeleteConfirmationOpen(false);
        void navigate(historyHref);
      },
    });
  }

  function duplicateRun() {
    const originalRequest = run.data?.original_request;
    if (!originalRequest || !run.data) return;

    const duplicateState: DuplicateRunState = {
      duplicateRequest: originalRequest,
      duplicateFromRunId: run.data.id,
    };
    void navigate("/runs/new", { state: duplicateState });
  }

  function requestWrite(result: ValidRunResult) {
    writeRun.reset();
    revertRun.reset();
    setPidActionAlert(null);
    setPidAction({ kind: "write", result });
  }

  function requestRevert(write: RunWrite) {
    writeRun.reset();
    revertRun.reset();
    setPidActionAlert(null);
    setPidAction({ kind: "revert", write });
  }

  function closePidAction() {
    if (pidActionPending) return;
    setPidAction(null);
    writeRun.reset();
    revertRun.reset();
  }

  function confirmPidAction() {
    if (!pidAction || pidActionPending) return;

    if (pidAction.kind === "write") {
      const responseLevel = pidAction.result.response_level;
      setPidAction(null);
      writeRun.mutate(
        {
          id: runId,
          responseLevel,
        },
        {
          onSuccess: (data) => {
            const latestWrite = data.writes.at(-1);
            if (
              latestWrite?.kind === "write" &&
              latestWrite.response_level === responseLevel &&
              !latestWrite.success
            ) {
              setPidActionAlert(
                `${RESPONSE_LEVEL_LABELS[responseLevel]}: ${writeFailureMessage(latestWrite)}`,
              );
            }
          },
          onError: (error) => {
            setPidActionAlert(
              userFacingErrorMessage(error, "Unable to apply PID settings."),
            );
          },
        },
      );
    } else {
      const responseLevel = pidAction.write.response_level;
      setPidAction(null);
      revertRun.mutate(runId, {
        onSuccess: (data) => {
          const latestWrite = data.writes.at(-1);
          if (
            latestWrite?.kind === "revert" &&
            latestWrite.response_level === responseLevel &&
            !latestWrite.success
          ) {
            setPidActionAlert(
              `${RESPONSE_LEVEL_LABELS[responseLevel]}: ${writeFailureMessage(latestWrite)}`,
            );
          }
        },
        onError: (error) => {
          setPidActionAlert(
            userFacingErrorMessage(
              error,
              "Unable to restore the previous PID settings.",
            ),
          );
        },
      });
    }
  }

  function handleNotesChange(value: string) {
    setNotes(value);
    setNotesDirty(true);
  }

  const errors: readonly RunErrorItem[] = [
    {
      key: "run",
      error: run.error,
      fallback: "Unable to load tune details.",
    },
    {
      key: "cancel",
      error: cancelRun.error,
      fallback: "Unable to cancel the tune.",
    },
    {
      key: "delete",
      error: deleteConfirmationOpen ? undefined : deleteRun.error,
      fallback: "Unable to delete the tune.",
    },
    {
      key: "save-notes",
      error: updateNotes.error,
      fallback: "Unable to save notes.",
    },
    {
      key: "clear-notes",
      error: deleteNotes.error,
      fallback: "Unable to clear notes.",
    },
  ];
  const showPidModal = shouldShowPidActionModal(
    run.data !== undefined,
    isDemo,
    capabilities.actions.write_pid,
    capabilities.actions.revert_pid,
  );

  return (
    <div>
      <RunStatusAnnouncement run={run.data} />
      <RunDetailActions
        id={id}
        runId={runId}
        historyHref={historyHref}
        demo={isDemo}
        actions={capabilities.actions}
        isRunning={isRunning}
        hasSamples={hasSamples}
        originalRequest={run.data?.original_request}
        cancelPending={cancelRun.isPending}
        deletePending={deleteRun.isPending}
        duplicateTitle={duplicateRunTitle(run.data, run.isSuccess)}
        onCancel={() => cancelRun.mutate(runId)}
        onDelete={handleDelete}
        onDuplicate={duplicateRun}
      />

      {!Number.isFinite(runId) && (
        <ErrorBanner message={`"${id}" is not a valid run id.`} />
      )}
      {run.isPending && Number.isFinite(runId) && (
        <LoadingState message="Loading run…" />
      )}
      <RunDetailErrors errors={errors} demo={isDemo} />
      {pidActionAlert && <ErrorBanner message={pidActionAlert} />}

      {run.isSuccess && (
        <RunDetailContent
          run={run.data}
          demo={isDemo}
          isRunning={isRunning}
          stream={stream}
          initialReadings={initialReadings}
          trendSamples={trendSamples}
          trendPoints={trendPoints}
          trendPollIntervalMs={trendPollIntervalMs}
          eligibility={eligibility}
          canRevertLastWrite={canRevert}
          notes={notes}
          notesDirty={notesDirty}
          savePending={updateNotes.isPending}
          clearPending={deleteNotes.isPending}
          writePending={writeRun.isPending}
          writingResponseLevel={writeRun.variables?.responseLevel}
          revertPending={revertRun.isPending}
          onNotesChange={handleNotesChange}
          onSaveNotes={saveNotes}
          onClearNotes={clearNotes}
          onWrite={requestWrite}
          onRevert={requestRevert}
        />
      )}
      {showPidModal && run.data && (
        <PidActionModal
          run={run.data}
          action={pidAction}
          pending={pidActionPending}
          error={pidActionError}
          onClose={closePidAction}
          onConfirm={confirmPidAction}
        />
      )}
      {deleteConfirmationOpen && (
        <ConfirmModal
          title="Delete tune?"
          onCancel={cancelDelete}
          onConfirm={confirmDelete}
          pending={deleteRun.isPending}
          confirmLabel="Delete tune"
          pendingLabel="Deleting tune…"
          errorMessage={
            deleteRun.isError
              ? userFacingErrorMessage(
                  deleteRun.error,
                  "Unable to delete the tune.",
                )
              : null
          }
          documentationId="history.delete-confirmation"
        >
          <p>
            Delete tune <strong>#{runId}</strong>? This cannot be undone.
          </p>
          <p className="mt-2 text-slate-400">
            Its recorded measurements, calculated results, and PID write history
            will be removed.
          </p>
        </ConfirmModal>
      )}
    </div>
  );
}
