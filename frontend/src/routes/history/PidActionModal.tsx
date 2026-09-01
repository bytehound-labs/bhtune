import type { RunDetailResponse } from "../../api/runs";
import { userFacingErrorMessage } from "../../api/errors";
import { RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";
import { Badge, Button, ErrorBanner, Modal } from "../../components/ui";
import {
  formatNumber,
  type ValidRunResult,
  type RunWrite,
} from "./runDetailHelpers";

export type PidAction =
  | {
      readonly kind: "write";
      readonly result: ValidRunResult;
    }
  | {
      readonly kind: "revert";
      readonly write: RunWrite;
    };

interface PidActionModalProps {
  readonly run: RunDetailResponse;
  readonly action: PidAction | null;
  readonly pending: boolean;
  readonly error: unknown;
  readonly onClose: () => void;
  readonly onConfirm: () => void;
}

export function PidActionModal({
  run,
  action,
  pending,
  error,
  onClose,
  onConfirm,
}: PidActionModalProps) {
  if (!action) return null;

  const isWrite = action.kind === "write";
  const tags = run.pid_constant_tags;
  const pidLabels = run.pid_parameter_labels ?? {
    proportional: "P",
    integral: "I",
    derivative: "D",
  };
  const values = isWrite
    ? {
        proportional: action.result.proportional,
        integral: action.result.integral,
        derivative: action.result.derivative,
      }
    : {
        proportional: action.write.proportional_previous,
        integral: action.write.integral_previous,
        derivative: action.write.derivative_previous,
      };
  const title = isWrite ? "Review PID settings" : "Review PID restore";
  const responseLevel = isWrite
    ? action.result.response_level
    : action.write.response_level;
  const primaryLabel = isWrite
    ? "Write PID settings"
    : "Restore previous values";
  const pendingLabel = isWrite
    ? "Writing and verifying…"
    : "Restoring and verifying…";
  const fallbackError = isWrite
    ? "Unable to apply PID settings."
    : "Unable to restore the previous PID settings.";

  return (
    <Modal
      title={title}
      onClose={onClose}
      dismissible={!pending}
      widthClassName="max-w-2xl"
    >
      <div className="space-y-5">
        <div className="rounded-lg border border-amber-700/70 bg-amber-950/40 p-4">
          <div className="flex gap-3">
            <span
              aria-hidden="true"
              className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-amber-600 text-sm font-bold text-amber-300"
            >
              !
            </span>
            <div className="text-sm">
              <p className="font-semibold text-amber-200">
                This action changes a live controller.
              </p>
              <p className="mt-1 text-amber-100/80">
                Review every destination tag and value before continuing. BHTune
                reads each value back and records the result.
              </p>
            </div>
          </div>
        </div>

        <dl className="grid gap-3 rounded-lg border border-slate-800 bg-slate-950/40 p-4 text-sm sm:grid-cols-2">
          <div>
            <dt className="text-xs uppercase tracking-wide text-slate-500">
              Loop tag
            </dt>
            <dd className="mt-1 break-all font-mono text-slate-200">
              {run.tag_name}
            </dd>
          </div>
          <div>
            <dt className="text-xs uppercase tracking-wide text-slate-500">
              Response level
            </dt>
            <dd className="mt-1">
              <Badge>{RESPONSE_LEVEL_LABELS[responseLevel]}</Badge>
            </dd>
          </div>
        </dl>

        {tags ? (
          <div className="overflow-hidden rounded-lg border border-slate-800">
            <table className="w-full text-left text-sm">
              <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                <tr>
                  <th className="px-4 py-2 font-medium">Parameter</th>
                  <th className="px-4 py-2 font-medium">Destination tag</th>
                  <th className="px-4 py-2 text-right font-medium">Value</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800">
                <PidValueRow
                  label={pidLabels.proportional}
                  tag={tags.proportional}
                  value={values.proportional}
                />
                <PidValueRow
                  label={pidLabels.integral}
                  tag={tags.integral}
                  value={values.integral}
                />
                <PidValueRow
                  label={pidLabels.derivative}
                  tag={tags.derivative}
                  value={values.derivative}
                />
              </tbody>
            </table>
          </div>
        ) : (
          <ErrorBanner message="The PID destination tags are unavailable for this tune." />
        )}

        {error != null && (
          <ErrorBanner message={userFacingErrorMessage(error, fallbackError)} />
        )}

        {pending && (
          <div
            role="status"
            aria-live="polite"
            className="flex items-center gap-3 rounded-lg border border-slate-700 bg-slate-950/60 px-4 py-3 text-sm text-slate-300"
          >
            <span
              aria-hidden="true"
              className="h-2.5 w-2.5 animate-pulse rounded-full bg-emerald-400"
            />
            <span>{pendingLabel} Do not close this dialog.</span>
          </div>
        )}

        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={pending} autoFocus={!pending}>
            Cancel
          </Button>
          <Button
            variant={isWrite ? "primary" : "danger"}
            disabled={pending || !tags}
            title={!tags ? "PID destination tags are unavailable." : undefined}
            onClick={onConfirm}
          >
            {pending ? pendingLabel : primaryLabel}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function PidValueRow({
  label,
  tag,
  value,
}: {
  readonly label: string;
  readonly tag: string;
  readonly value: number | null | undefined;
}) {
  return (
    <tr>
      <td className="px-4 py-3 font-medium text-slate-300">{label}</td>
      <td className="break-all px-4 py-3 font-mono text-slate-400">{tag}</td>
      <td className="px-4 py-3 text-right font-mono text-slate-100">
        {formatNumber(value)}
      </td>
    </tr>
  );
}
