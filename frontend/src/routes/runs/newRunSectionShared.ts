import type { SimulatorCapabilities } from "../../api/capabilities";
import type {
  ControllerDirection,
  NumOrBlank,
  TagMappingSource,
  TagOverrideKey,
  ValueMappingKey,
  ValueMappingSource,
} from "./mappingState";
import type {
  FormState,
  ProcessType,
  TemplateResponse,
  TuneDriver,
} from "./newRunFormState";

export type NewRunSectionProps = {
  readonly simulatorCapabilities: SimulatorCapabilities | undefined;
  readonly form: FormState;
  readonly template: TemplateResponse | undefined;
  readonly templates: readonly TemplateResponse[] | undefined;
  readonly templatesPending: boolean;
  readonly onChange: <K extends keyof FormState>(
    key: K,
    value: FormState[K],
  ) => void;
  readonly onTagNameChange: (value: string) => void;
  readonly onDriverChange: (value: TuneDriver) => void;
  readonly onTemplateChange: (value: string) => void;
  readonly onProcessTypeChange: (value: ProcessType) => void;
  readonly onResetProcessDefaults: () => void;
  readonly onTagSourceChange: (
    key: TagOverrideKey,
    source: TagMappingSource,
  ) => void;
  readonly onTagChange: (key: TagOverrideKey, value: string) => void;
  readonly onValueSourceChange: (
    key: ValueMappingKey,
    source: ValueMappingSource,
  ) => void;
  readonly onValueTagChange: (key: ValueMappingKey, value: string) => void;
  readonly onValueChange: (
    key:
      | "opcDirection"
      | "opcPvRangeHigh"
      | "opcPvRangeLow"
      | "opcMvRangeHigh"
      | "opcMvRangeLow"
      | "simDirection"
      | "simPvRangeHigh"
      | "simPvRangeLow"
      | "simMvRangeHigh"
      | "simMvRangeLow",
    value: NumOrBlank | ControllerDirection,
  ) => void;
  readonly onResetTag: (key: TagOverrideKey) => void;
  readonly onResetValue: (key: ValueMappingKey) => void;
  readonly onResetAll: () => void;
  readonly onOpenTagBrowser: () => void;
};

export function templateHint(driver: TuneDriver): string {
  if (driver === "simulator") {
    return "The selected template formats calculated PID constants using that system's native conventions (for example, gain versus proportional band).";
  }
  return "Maps the connected DCS/PLC's item IDs and PID conventions.";
}

export function disabledGatewayHint(driver: TuneDriver): string | undefined {
  if (driver === "simulator") {
    return "Disabled — the simulator never contacts a gateway.";
  }
  return "opcda-bridge gateway address (host:port).";
}

export function tagNameHint(driver: TuneDriver): string {
  if (driver === "simulator") {
    return "Disabled — the simulator hardcodes its own PV/MV tags and ignores this.";
  }
  return "PV tag prefix; the rest of the tag set is derived from it via the template's suffixes.";
}
