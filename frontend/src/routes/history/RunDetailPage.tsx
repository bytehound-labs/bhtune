import { Link, useNavigate, useParams } from "react-router";
import {
  runExportUrl,
  useCancelRun,
  useDeleteRun,
  useRun,
  useRunStream,
} from "../../api/runs";
import {
  CONTROLLER_TYPE_LABELS,
  DIRECTION_LABELS,
  DRIVER_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import {
  Badge,
  Button,
  ErrorBanner,
  Field,
  LoadingState,
  PageHeading,
  Section,
} from "../../components/ui";
import { TrendChart } from "../../components/TrendChart";

const originTone = {
  builtin: "success",
  catalog: "neutral",
  user: "warning",
} as const;

const outcomeTone = {
  running: "neutral",
  completed: "success",
  failed: "error",
  aborted: "warning",
} as const;

const restoreTone = {
  confirmed: "success",
  incomplete: "warning",
} as const;

/** Trims a float to a stable, readable precision without trailing zeros. */
function num(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return String(Number(value.toFixed(4)));
}

function dateTime(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "—";
}

export function RunDetailPage() {
  const { id } = useParams<{ id: string }>();
  const runId = Number(id);
  const navigate = useNavigate();
  const run = useRun(runId);
  const cancelRun = useCancelRun();
  const deleteRun = useDeleteRun();
  const isRunning = run.data?.outcome === "running";
  const hasSamples = run.data ? run.data.samples.length > 0 : false;
  const stream = useRunStream(runId, isRunning);
  // While running, the live SSE feed is the source of truth (it replays every sample from
  // tick 0, so it's a complete trend on its own); once terminal, fall back to `useRun`'s
  // plain REST `samples`, which is cheaper than keeping a stream open for a run that's
  // already over.
  const trendSamples = isRunning ? stream.samples : (run.data?.samples ?? []);

  return (
    <div>
      <PageHeading
        title={`Run #${id ?? ""}`}
        actions={
          <>
            {isRunning && (
              <Button
                variant="danger"
                disabled={cancelRun.isPending}
                onClick={() => cancelRun.mutate(runId)}
              >
                {cancelRun.isPending ? "Cancelling…" : "Cancel run"}
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
                disabled={deleteRun.isPending}
                onClick={() => {
                  if (
                    window.confirm(
                      `Delete run #${runId}? This removes its samples, results, and write-back audit rows and cannot be undone.`,
                    )
                  ) {
                    deleteRun.mutate(runId, {
                      onSuccess: () => navigate("/runs"),
                    });
                  }
                }}
              >
                {deleteRun.isPending ? "Deleting…" : "Delete run"}
              </Button>
            )}
            <Link to="/runs">
              <Button>Back to history</Button>
            </Link>
          </>
        }
      />

      {!Number.isFinite(runId) && (
        <ErrorBanner message={`"${id}" is not a valid run id.`} />
      )}
      {run.isPending && Number.isFinite(runId) && (
        <LoadingState message="Loading run…" />
      )}
      {run.isError && <ErrorBanner message={run.error.message} />}
      {cancelRun.isError && <ErrorBanner message={cancelRun.error.message} />}
      {deleteRun.isError && <ErrorBanner message={deleteRun.error.message} />}

      {run.isSuccess && (
        <>
          {isRunning && (
            <div className="mb-6 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm">
              <p className="text-slate-300">
                Run in progress — streaming live via SSE.
                {stream.reconnecting && (
                  <span className="ml-2 text-amber-400">Reconnecting…</span>
                )}
              </p>
              {stream.samples.length > 0 ? (
                (() => {
                  const latest = stream.samples[stream.samples.length - 1];
                  return (
                    <p className="mt-2 font-mono text-slate-400">
                      Tick {latest.tick_index}: PV {num(latest.sample.pv)}, MV{" "}
                      {num(latest.state.mv_value_current)}, cycles{" "}
                      {latest.state.cycles_completed} completed /{" "}
                      {latest.state.cycles_remaining} remaining
                    </p>
                  );
                })()
              ) : (
                <p className="mt-2 text-slate-500">No samples recorded yet.</p>
              )}
            </div>
          )}

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Trend
            </h2>
            {trendSamples.length === 0 ? (
              <p className="text-sm text-slate-500">
                No samples recorded yet for this run.
              </p>
            ) : (
              <TrendChart samples={trendSamples} />
            )}
          </section>

          <Section title="Summary">
            <Field label="Loop" value={run.data.loop_name} />
            <Field
              label="Outcome"
              value={
                <Badge tone={outcomeTone[run.data.outcome]}>
                  {OUTCOME_LABELS[run.data.outcome]}
                </Badge>
              }
            />
            <Field label="Driver" value={DRIVER_LABELS[run.data.driver]} />
            <Field
              label="Template"
              value={
                <>
                  {run.data.template_name}{" "}
                  <Badge tone={originTone[run.data.template_origin]}>
                    {run.data.template_origin}
                  </Badge>
                </>
              }
            />
            <Field label="Started" value={dateTime(run.data.started_at)} />
            <Field label="Completed" value={dateTime(run.data.completed_at)} />
            {run.data.failure_reason && (
              <Field
                label="Failure reason"
                value={run.data.failure_reason}
                full
              />
            )}
            {run.data.restore_status && (
              <Field
                label="Restore status"
                value={
                  <Badge tone={restoreTone[run.data.restore_status]}>
                    {run.data.restore_status}
                  </Badge>
                }
              />
            )}
            {run.data.restore_detail && (
              <Field
                label="Restore detail"
                value={run.data.restore_detail}
                full
              />
            )}
          </Section>

          <Section title="Test configuration">
            <Field
              label="Process type"
              value={PROCESS_TYPE_LABELS[run.data.config.process_type]}
            />
            <Field
              label="Controller type"
              value={CONTROLLER_TYPE_LABELS[run.data.config.controller_type]}
            />
            <Field
              label="Relay amplitude"
              value={`${num(run.data.config.relay_amp_percent)}%`}
            />
            <Field
              label="Cycles (skip / count)"
              value={`${run.data.config.num_cycles_skip} / ${run.data.config.num_cycles_count}`}
            />
            <Field
              label="Noise protection"
              value={`${run.data.config.noise_protection_secs}s`}
            />
            <Field
              label="MRFT delay padding"
              value={`${run.data.config.mrft_delay_secs}s`}
            />
          </Section>

          {run.data.initial_readings && (
            <Section title="Initial readings">
              <Field
                label="PV initial"
                value={num(run.data.initial_readings.pv_ini)}
              />
              <Field
                label="MV initial"
                value={num(run.data.initial_readings.mv_ini)}
              />
              <Field
                label="PV range"
                value={`${num(run.data.initial_readings.pv_range_low)} – ${num(run.data.initial_readings.pv_range_high)}`}
              />
              <Field
                label="MV range"
                value={`${num(run.data.initial_readings.mv_range_low)} – ${num(run.data.initial_readings.mv_range_high)}`}
              />
              <Field
                label="Controller direction"
                value={
                  DIRECTION_LABELS[
                    run.data.initial_readings.controller_direction
                  ]
                }
              />
              <Field
                label="Setpoint initial"
                value={num(run.data.initial_readings.setpoint_ini)}
              />
              <Field
                label="Mode (raw)"
                value={run.data.initial_readings.mode_raw ?? "—"}
              />
              <Field
                label="Mode attribute (raw)"
                value={run.data.initial_readings.mode_attribute_raw ?? "—"}
              />
            </Section>
          )}

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Calculated results
            </h2>
            {run.data.results.length === 0 ? (
              <p className="text-sm text-slate-500">
                No results were calculated for this run.
              </p>
            ) : (
              <div className="overflow-hidden rounded-lg border border-slate-800">
                <table className="w-full text-left text-sm">
                  <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                    <tr>
                      <th className="px-4 py-2 font-medium">Response level</th>
                      <th className="px-4 py-2 font-medium">Kp</th>
                      <th className="px-4 py-2 font-medium">Ti (min)</th>
                      <th className="px-4 py-2 font-medium">Td (min)</th>
                      <th className="px-4 py-2 font-medium">P</th>
                      <th className="px-4 py-2 font-medium">I</th>
                      <th className="px-4 py-2 font-medium">D</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800">
                    {run.data.results.map((result) => (
                      <tr key={result.response_level}>
                        <td className="px-4 py-3 font-medium">
                          {RESPONSE_LEVEL_LABELS[result.response_level]}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.kp)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.ti_minutes)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.td_minutes)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.proportional)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.integral)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.derivative)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Write-back audit
            </h2>
            {run.data.writes.length === 0 ? (
              <p className="text-sm text-slate-500">
                No PID constants were written back for this run.
              </p>
            ) : (
              <div className="overflow-x-auto rounded-lg border border-slate-800">
                <table className="w-full text-left text-sm">
                  <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                    <tr>
                      <th className="px-4 py-2 font-medium">Kind</th>
                      <th className="px-4 py-2 font-medium">Level</th>
                      <th className="px-4 py-2 font-medium">Written at</th>
                      <th className="px-4 py-2 font-medium">
                        Previous (P/I/D)
                      </th>
                      <th className="px-4 py-2 font-medium">Written (P/I/D)</th>
                      <th className="px-4 py-2 font-medium">
                        Readback (P/I/D)
                      </th>
                      <th className="px-4 py-2 font-medium">Success</th>
                      <th className="px-4 py-2 font-medium">Rollback</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800">
                    {run.data.writes.map((write, i) => (
                      <tr key={i}>
                        <td className="px-4 py-3">{write.kind}</td>
                        <td className="px-4 py-3">
                          {RESPONSE_LEVEL_LABELS[write.response_level]}
                        </td>
                        <td className="px-4 py-3 text-slate-400">
                          {dateTime(write.written_at)}
                        </td>
                        <td className="px-4 py-3 font-mono text-slate-400">
                          {num(write.proportional_previous)} /{" "}
                          {num(write.integral_previous)} /{" "}
                          {num(write.derivative_previous)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(write.proportional_written)} /{" "}
                          {num(write.integral_written)} /{" "}
                          {num(write.derivative_written)}
                        </td>
                        <td className="px-4 py-3 font-mono text-slate-400">
                          {num(write.proportional_readback)} /{" "}
                          {num(write.integral_readback)} /{" "}
                          {num(write.derivative_readback)}
                        </td>
                        <td className="px-4 py-3">
                          <Badge tone={write.success ? "success" : "error"}>
                            {write.success ? "ok" : "failed"}
                          </Badge>
                        </td>
                        <td className="px-4 py-3">
                          {write.rollback_state ? (
                            <Badge
                              tone={
                                write.rollback_state === "succeeded"
                                  ? "success"
                                  : "error"
                              }
                            >
                              {write.rollback_state}
                            </Badge>
                          ) : (
                            <span className="text-slate-500">—</span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {run.data.writes.some((w) => w.error_message) && (
              <div className="mt-3 space-y-2">
                {run.data.writes
                  .filter((w) => w.error_message)
                  .map((w, i) => (
                    <ErrorBanner
                      key={i}
                      message={`${RESPONSE_LEVEL_LABELS[w.response_level]}: ${w.error_message}`}
                    />
                  ))}
              </div>
            )}
          </section>

          <p className="text-sm text-slate-500">
            {trendSamples.length} per-tick samples{" "}
            {isRunning ? "recorded so far" : "were recorded"} for this run.
          </p>
        </>
      )}
    </div>
  );
}
