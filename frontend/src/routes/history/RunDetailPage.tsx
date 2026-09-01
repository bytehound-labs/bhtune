import { useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router";
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
  type SampleResponse,
} from "../../api/runs";
import type { DuplicateRunState } from "../runs/NewRunPage";
import { composeTrendPoints } from "../../lib/trend";
import {
  type RunResult,
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
import { ErrorBanner, LoadingState } from "../../components/ui";
import { RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";

const EMPTY_TREND_SAMPLES: readonly SampleResponse[] = [];

export function RunDetailPage() {
  const { id } = useParams<{ id: string }>();
  const runId = Number(id);
  const navigate = useNavigate();
  const run = useRun(runId);
  const cancelRun = useCancelRun();
  const updateNotes = useUpdateRunNotes();
  const deleteNotes = useDeleteRunNotes();
  const deleteRun = useDeleteRun();
  const writeRun = useWriteRun();
  const revertRun = useRevertRun();
  const isRunning = run.data?.outcome === "running";
  const hasSamples = run.data ? run.data.samples.length > 0 : false;
  const eligibility = run.data
    ? writeEligibility(run.data)
    : { eligible: false };
  const writes = run.data?.writes ?? [];
  const lastWrite = writes.at(-1);
  // Restore always targets the newest WriteKind::Write row server-side. Only offer it
  // while that row is still newest, so a superseded restore action cannot mislead.
  const canRevertLastWrite =
    eligibility.eligible &&
    lastWrite !== undefined &&
    lastWrite.kind === "write" &&
    lastWrite.success;
  const stream = useRunStream(runId, isRunning);
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
  const [notes, setNotes] = useState("");
  const [notesDirty, setNotesDirty] = useState(false);
  const [pidAction, setPidAction] = useState<PidAction | null>(null);
  const [pidActionAlert, setPidActionAlert] = useState<string | null>(null);
  const pidActionPending = writeRun.isPending || revertRun.isPending;
  const pidActionError = (() => {
    if (!pidAction) return null;
    if (pidAction.kind === "write") return writeRun.error;
    return revertRun.error;
  })();

  useEffect(() => {
    if (run.data?.id === runId) {
      setNotes(run.data.notes ?? "");
      setNotesDirty(false);
    }
  }, [runId, run.data?.id, run.data?.notes]);

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
    const confirmed = window.confirm(
      `Delete tune #${runId}? This removes its recorded measurements and results and cannot be undone.`,
    );
    if (confirmed) {
      deleteRun.mutate(runId, {
        onSuccess: () => navigate("/runs"),
      });
    }
  }

  function duplicateRun() {
    const originalRequest = run.data?.original_request;
    if (!originalRequest || !run.data) return;

    const duplicateState: DuplicateRunState = {
      duplicateRequest: originalRequest,
      duplicateFromRunId: run.data.id,
    };
    navigate("/runs/new", { state: duplicateState });
  }

  function requestWrite(result: RunResult) {
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
      error: deleteRun.error,
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

  return (
    <div>
      <RunDetailActions
        id={id}
        runId={runId}
        isRunning={isRunning}
        hasSamples={hasSamples}
        originalRequest={run.data?.original_request}
        cancelPending={cancelRun.isPending}
        deletePending={deleteRun.isPending}
        duplicateTitle={
          run.isSuccess && !run.data.original_request
            ? "This tune's original settings weren't recorded and can't be duplicated."
            : undefined
        }
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
      <RunDetailErrors errors={errors} />
      {pidActionAlert && <ErrorBanner message={pidActionAlert} />}

      {run.isSuccess && (
        <RunDetailContent
          run={run.data}
          isRunning={isRunning}
          stream={stream}
          initialReadings={initialReadings}
          trendSamples={trendSamples}
          trendPoints={trendPoints}
          trendPollIntervalMs={trendPollIntervalMs}
          eligibility={eligibility}
          canRevertLastWrite={canRevertLastWrite}
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
      {run.data && (
        <PidActionModal
          run={run.data}
          action={pidAction}
          pending={pidActionPending}
          error={pidActionError}
          onClose={closePidAction}
          onConfirm={confirmPidAction}
        />
      )}
    </div>
  );
}
