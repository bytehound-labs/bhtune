import {
  SAMPLE_QUALITY_LABELS,
  SAMPLE_QUALITY_TONE,
} from "../../lib/enumLabels";
import { Badge, Button } from "../ui";
import type { QualityWarning } from "./browseModel";

type QualityWarningPanelProps = Readonly<{
  warning: QualityWarning;
  onChooseDifferent: () => void;
  onProceed: () => void;
}>;

export function QualityWarningPanel({
  warning,
  onChooseDifferent,
  onProceed,
}: QualityWarningPanelProps) {
  return (
    <div className="space-y-4">
      <div className="rounded-md border border-amber-800 bg-amber-950/50 p-3 text-sm text-amber-200">
        <p className="font-medium">This tag returned a non-Good OPC quality.</p>
        <p className="mt-2">
          The live value for{" "}
          <span className="font-mono">{warning.selectedTag}</span> was{" "}
          <span className="font-mono">{warning.reading.value}</span> with
          quality{" "}
          <Badge tone={SAMPLE_QUALITY_TONE[warning.reading.quality]}>
            {SAMPLE_QUALITY_LABELS[warning.reading.quality]}
          </Badge>
          .
        </p>
        <p className="mt-2">
          Non-Good values may be stale or invalid. Choose another tag, or
          proceed anyway if you understand the risk.
        </p>
        <p className="mt-2">
          Proceeding only selects this item for the form; a tune still requires
          trustworthy quality for its live readings.
        </p>
      </div>
      <div className="flex justify-end gap-2">
        <Button onClick={onChooseDifferent}>Choose a different tag</Button>
        <Button variant="primary" onClick={onProceed}>
          Proceed anyway
        </Button>
      </div>
    </div>
  );
}
