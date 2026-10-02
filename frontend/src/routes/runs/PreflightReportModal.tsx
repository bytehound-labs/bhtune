import type { PreflightResponse } from "../../api/runs";
import { userFacingErrorMessage } from "../../api/errors";
import {
  Badge,
  Button,
  ErrorBanner,
  LoadingStatus,
  Modal,
} from "../../components/ui";

function statusTone(
  status: PreflightResponse["status"],
): "success" | "warning" | "error" {
  switch (status) {
    case "pass":
      return "success";
    case "warn":
      return "warning";
    case "fail":
      return "error";
  }
}

export function PreflightReportModal({
  open,
  pending,
  report,
  error,
  onClose,
}: {
  readonly open: boolean;
  readonly pending: boolean;
  readonly report: PreflightResponse | undefined;
  readonly error: unknown;
  readonly onClose: () => void;
}) {
  if (!open) return null;

  return (
    <Modal
      title="Readiness check"
      onClose={onClose}
      dismissible={!pending}
      widthClassName="max-w-4xl"
      documentationId="new-tune.preflight"
    >
      <div className="space-y-5">
        <p className="text-sm text-slate-300">
          This read-only check does not start a tune, create run history, or
          write controller values.
        </p>

        {pending && (
          <LoadingStatus
            message="Checking tune readiness…"
            className="rounded-lg border border-slate-700 bg-slate-950/60 px-4 py-3 text-sm text-slate-300"
          />
        )}

        {error != null && (
          <ErrorBanner
            message={userFacingErrorMessage(
              error,
              "Unable to check tune readiness.",
            )}
          />
        )}

        {report && (
          <>
            <div
              role="status"
              aria-live="polite"
              className="flex items-center gap-3 rounded-lg border border-slate-800 bg-slate-950/40 px-4 py-3"
            >
              <span className="text-sm font-medium text-slate-300">
                Overall result
              </span>
              <Badge tone={statusTone(report.status)}>{report.status}</Badge>
            </div>

            <section aria-labelledby="preflight-checks-heading">
              <h3
                id="preflight-checks-heading"
                className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400"
              >
                Checks
              </h3>
              <ul className="space-y-2">
                {report.checks.map((check) => (
                  <li
                    key={check.name}
                    className="rounded-lg border border-slate-800 bg-slate-950/40 p-3"
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <h4 className="text-sm font-medium text-slate-200">
                        {check.name}
                      </h4>
                      <Badge tone={statusTone(check.status)}>
                        {check.status}
                      </Badge>
                    </div>
                    <p className="mt-1 break-words text-sm text-slate-400">
                      {check.detail}
                    </p>
                  </li>
                ))}
              </ul>
            </section>

            <section aria-labelledby="preflight-tag-reads-heading">
              <h3
                id="preflight-tag-reads-heading"
                className="mb-2 text-sm font-semibold uppercase tracking-wide text-slate-400"
              >
                Tag reads
              </h3>
              {report.tag_reads.length > 0 ? (
                <div className="overflow-x-auto rounded-lg border border-slate-800">
                  <table className="w-full min-w-[48rem] text-left text-sm">
                    <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                      <tr>
                        <th className="px-3 py-2 font-medium">Tag</th>
                        <th className="px-3 py-2 font-medium">Roles</th>
                        <th className="px-3 py-2 font-medium">Value</th>
                        <th className="px-3 py-2 font-medium">Quality</th>
                        <th className="px-3 py-2 font-medium">Status</th>
                        <th className="px-3 py-2 font-medium">Detail</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-slate-800">
                      {report.tag_reads.map((read) => (
                        <tr key={read.tag}>
                          <td className="break-all px-3 py-2 font-mono text-slate-300">
                            {read.tag}
                          </td>
                          <td className="px-3 py-2 text-slate-400">
                            {read.roles.join(", ")}
                          </td>
                          <td className="break-all px-3 py-2 font-mono text-slate-300">
                            {read.value ?? "—"}
                          </td>
                          <td className="px-3 py-2 text-slate-300">
                            {read.quality ?? "—"}
                          </td>
                          <td className="px-3 py-2">
                            <Badge tone={statusTone(read.status)}>
                              {read.status}
                            </Badge>
                          </td>
                          <td className="break-words px-3 py-2 text-slate-400">
                            {read.detail}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : (
                <p className="text-sm text-slate-400">
                  No tag reads were needed for this request.
                </p>
              )}
            </section>
          </>
        )}

        <div className="flex justify-end">
          <Button onClick={onClose} disabled={pending}>
            Close
          </Button>
        </div>
      </div>
    </Modal>
  );
}
