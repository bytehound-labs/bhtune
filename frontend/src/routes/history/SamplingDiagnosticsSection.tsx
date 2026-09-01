import type { RunDetailResponse } from "../../api/runs";
import {
  SAMPLING_ADEQUACY_LABELS,
  SAMPLING_ADEQUACY_TONE,
} from "../../lib/enumLabels";
import { Badge, CollapsibleSection } from "../../components/ui";

export function SamplingDiagnosticsSection({
  timing,
}: {
  readonly timing: RunDetailResponse["timing_metrics"];
}) {
  if (!timing) return null;

  const samplingAdequacy = timing.sampling_adequacy ?? "not_assessed";

  return (
    <CollapsibleSection title="Sampling diagnostics" defaultOpen={false}>
      <div
        className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-3"
        aria-label="Sampling adequacy advisory"
      >
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-xs font-semibold uppercase tracking-wide text-slate-400">
            Sampling adequacy
          </span>
          <Badge tone={SAMPLING_ADEQUACY_TONE[samplingAdequacy]}>
            {SAMPLING_ADEQUACY_LABELS[samplingAdequacy]}
          </Badge>
        </div>
        <p className="mt-2 text-sm text-slate-400">
          {samplingAdvisoryText(
            samplingAdequacy,
            timing.approximate_samples_per_period,
          )}
        </p>
      </div>
    </CollapsibleSection>
  );
}

function samplingAdvisoryText(
  adequacy: "adequate" | "marginal" | "not_assessed",
  samplesPerPeriod: number | null | undefined,
): string {
  switch (adequacy) {
    case "adequate":
      return `At least six samples per oscillation period were observed${formatSamplesPerPeriod(samplesPerPeriod)}.`;
    case "marginal":
      return `Fewer than six samples per oscillation period were observed${formatSamplesPerPeriod(samplesPerPeriod)}; calculated values may be less reliable.`;
    case "not_assessed":
      return "Sampling adequacy was not assessed because no finite oscillation period was available.";
  }
}

function formatSamplesPerPeriod(value: number | null | undefined): string {
  return value === null || value === undefined
    ? ""
    : ` (${value.toFixed(2)} observed)`;
}
