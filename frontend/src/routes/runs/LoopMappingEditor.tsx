import type { components } from "../../api/schema";
import {
  Button,
  NumberField,
  SelectField,
  TextField,
} from "../../components/ui";
import { DIRECTION_LABELS } from "../../lib/enumLabels";
import { derivedTagPreview } from "../../lib/opcTags";
import {
  type ControllerDirection,
  type NumOrBlank,
  type TagMappingSource,
  type TagOverrideFormState,
  type TagOverrideKey,
  type TagMappingSources,
  type TuneDriver,
  type ValueMappingKey,
  type ValueMappingSource,
  type ValueMappingSources,
} from "./mappingState";

type TemplateResponse = components["schemas"]["TemplateResponse"];

type TagRow = {
  key: TagOverrideKey;
  label: string;
  previewLabel: string;
};

const TAG_ROWS: readonly TagRow[] = [
  {
    key: "processVariable",
    label: "Process variable (PV)",
    previewLabel: "Process variable (PV)",
  },
  {
    key: "manipulatedVariable",
    label: "Manipulated variable (MV)",
    previewLabel: "Manipulated variable (MV)",
  },
  {
    key: "setpointVariable",
    label: "Setpoint",
    previewLabel: "Setpoint",
  },
  {
    key: "controllerMode",
    label: "Controller mode",
    previewLabel: "Controller mode",
  },
  {
    key: "modeAttribute",
    label: "Mode attribute",
    previewLabel: "Mode attribute",
  },
  {
    key: "proportionalConstant",
    label: "Proportional constant",
    previewLabel: "Proportional constant",
  },
  {
    key: "integralConstant",
    label: "Integral constant",
    previewLabel: "Integral constant",
  },
  {
    key: "derivativeConstant",
    label: "Derivative constant",
    previewLabel: "Derivative constant",
  },
];

type ValueRow = {
  key: ValueMappingKey;
  label: string;
  previewLabel: string;
  kind: "direction" | "number";
};

const VALUE_ROWS: readonly ValueRow[] = [
  {
    key: "direction",
    label: "Controller direction",
    previewLabel: "Controller direction",
    kind: "direction",
  },
  {
    key: "pvRangeHigh",
    label: "PV range high",
    previewLabel: "PV range high",
    kind: "number",
  },
  {
    key: "pvRangeLow",
    label: "PV range low",
    previewLabel: "PV range low",
    kind: "number",
  },
  {
    key: "mvRangeHigh",
    label: "MV range high",
    previewLabel: "MV range high",
    kind: "number",
  },
  {
    key: "mvRangeLow",
    label: "MV range low",
    previewLabel: "MV range low",
    kind: "number",
  },
];

type MappingValueState = {
  driver: TuneDriver;
  tagname: string;
  tagOverrides: TagOverrideFormState;
  tagSources: TagMappingSources;
  valueSources: ValueMappingSources;
  opcDirection: "" | ControllerDirection;
  opcPvRangeHigh: NumOrBlank;
  opcPvRangeLow: NumOrBlank;
  opcMvRangeHigh: NumOrBlank;
  opcMvRangeLow: NumOrBlank;
  simDirection: "" | ControllerDirection;
  simPvRangeHigh: NumOrBlank;
  simPvRangeLow: NumOrBlank;
  simMvRangeHigh: NumOrBlank;
  simMvRangeLow: NumOrBlank;
};

type Props = {
  state: MappingValueState;
  template: TemplateResponse | undefined;
  onTagSourceChange: (key: TagOverrideKey, source: TagMappingSource) => void;
  onTagChange: (key: TagOverrideKey, value: string) => void;
  onValueSourceChange: (
    key: ValueMappingKey,
    source: ValueMappingSource,
  ) => void;
  onValueChange: (
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
    value: NumOrBlank | "" | ControllerDirection,
  ) => void;
  onResetTag: (key: TagOverrideKey) => void;
  onResetValue: (key: ValueMappingKey) => void;
  onResetAll: () => void;
};

function SourceToggle<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
}: {
  label: string;
  value: T;
  options: readonly { value: T; label: string; disabled?: boolean }[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <div
      className="inline-flex rounded-md border border-slate-700"
      role="group"
      aria-label={`${label} source`}
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={value === option.value}
          disabled={disabled || option.disabled}
          onClick={() => onChange(option.value)}
          className={`px-2.5 py-1 text-xs font-medium transition-colors first:rounded-l-md last:rounded-r-md ${
            value === option.value
              ? "bg-slate-700 text-slate-100"
              : "bg-slate-950 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
          } disabled:cursor-not-allowed disabled:opacity-50`}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

function displayTag(tag: string | null, missingTemplateMessage: string) {
  if (tag) {
    return (
      <span className="break-all font-mono text-sm text-slate-100">{tag}</span>
    );
  }
  return (
    <span className="text-sm italic text-slate-500">
      {missingTemplateMessage}
    </span>
  );
}

function tagValue(
  row: TagRow,
  state: MappingValueState,
  template: TemplateResponse | undefined,
) {
  const preview = template
    ? derivedTagPreview(state.tagname, template).find(
        (item) => item.label === row.previewLabel,
      )?.tag
    : null;
  const source = state.tagSources[row.key];
  const custom = state.tagOverrides[row.key].trim();
  return {
    preview,
    source,
    custom,
    effective: source === "custom" && custom ? custom : (preview ?? null),
  };
}

function valueState(
  key: ValueMappingKey,
  state: MappingValueState,
  template: TemplateResponse | undefined,
) {
  const preview = template
    ? derivedTagPreview(state.tagname, template).find(
        (item) =>
          item.label ===
          VALUE_ROWS.find((row) => row.key === key)?.previewLabel,
      )?.tag
    : null;
  const source = state.valueSources[key];
  if (state.driver === "simulator") {
    return {
      source: "fixed" as const,
      preview: preview ?? null,
      value:
        key === "direction"
          ? state.simDirection
          : key === "pvRangeHigh"
            ? state.simPvRangeHigh
            : key === "pvRangeLow"
              ? state.simPvRangeLow
              : key === "mvRangeHigh"
                ? state.simMvRangeHigh
                : state.simMvRangeLow,
    };
  }
  return {
    source,
    preview: preview ?? null,
    value:
      key === "direction"
        ? state.opcDirection
        : key === "pvRangeHigh"
          ? state.opcPvRangeHigh
          : key === "pvRangeLow"
            ? state.opcPvRangeLow
            : key === "mvRangeHigh"
              ? state.opcMvRangeHigh
              : state.opcMvRangeLow,
  };
}

function valueText(value: NumOrBlank | "" | ControllerDirection) {
  if (value === "") return "No fixed value";
  if (value === "direct" || value === "reverse") {
    return DIRECTION_LABELS[value];
  }
  return String(value);
}

export function LoopMappingEditor({
  state,
  template,
  onTagSourceChange,
  onTagChange,
  onValueSourceChange,
  onValueChange,
  onResetTag,
  onResetValue,
  onResetAll,
}: Props) {
  const simulator = state.driver === "simulator";

  return (
    <div className="sm:col-span-2">
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl text-sm text-slate-400">
          <p>
            Each row shows the value that will be used and where it comes from.
            Template/read-tag values follow the selected template and Tag name;
            custom/fixed values apply only to this tune.
          </p>
          {simulator && (
            <p className="mt-2 text-amber-300">
              Simulator mode does not use OPC tag rows. Its direction and ranges
              are separate simulator values and never become OPC DA overrides.
            </p>
          )}
        </div>
        <Button onClick={onResetAll}>Reset all mapping overrides</Button>
      </div>

      <div className="space-y-3">
        {TAG_ROWS.map((row) => {
          const tags = tagValue(row, state, template);
          const inactive = simulator;
          return (
            <div
              key={row.key}
              role="group"
              aria-label={row.label}
              className={`rounded-md border border-slate-800 p-3 ${
                inactive ? "opacity-65" : ""
              }`}
            >
              <div className="flex flex-col gap-3 lg:grid lg:grid-cols-[minmax(10rem,1fr)_auto_minmax(14rem,2fr)_auto] lg:items-start lg:gap-4">
                <div>
                  <div className="text-sm font-medium text-slate-200">
                    {row.label}
                  </div>
                  <div className="mt-1 text-xs text-slate-500">
                    {inactive
                      ? "Inactive for Simulator"
                      : tags.source === "custom"
                        ? "Custom tag"
                        : "Template default"}
                  </div>
                </div>
                <SourceToggle
                  label={row.label}
                  value={tags.source}
                  options={[
                    { value: "template", label: "Template" },
                    { value: "custom", label: "Custom" },
                  ]}
                  disabled={inactive}
                  onChange={(source) => onTagSourceChange(row.key, source)}
                />
                <div className="min-w-0">
                  {tags.source === "custom" ? (
                    <TextField
                      label={`${row.label} custom tag`}
                      value={state.tagOverrides[row.key]}
                      onChange={(value) => onTagChange(row.key, value)}
                      disabled={inactive}
                      placeholder={tags.preview ?? "Enter a custom OPC item ID"}
                      hint={
                        tags.preview
                          ? "Reset returns to the template-derived value."
                          : "This template has no default for this item; enter a site-specific tag if needed."
                      }
                    />
                  ) : (
                    <div>
                      <div className="text-xs uppercase tracking-wide text-slate-500">
                        Effective value
                      </div>
                      <div className="mt-1 min-h-8 rounded-md border border-slate-800 bg-slate-950 px-3 py-1.5">
                        {displayTag(
                          tags.effective,
                          template
                            ? "Not used by this template"
                            : "Choose a template",
                        )}
                      </div>
                    </div>
                  )}
                </div>
                <Button
                  onClick={() => onResetTag(row.key)}
                  disabled={inactive || tags.source === "template"}
                  title={
                    inactive
                      ? "Simulator mode does not use OPC tag overrides."
                      : tags.source === "template"
                        ? "This row already uses the template default."
                        : "Reset this row to the template default."
                  }
                >
                  Reset
                </Button>
              </div>
            </div>
          );
        })}

        {VALUE_ROWS.map((row) => {
          const values = valueState(row.key, state, template);
          const fixed = values.source === "fixed";
          const valueKey = simulator
            ? row.key === "direction"
              ? "simDirection"
              : `sim${row.key[0].toUpperCase()}${row.key.slice(1)}`
            : row.key === "direction"
              ? "opcDirection"
              : `opc${row.key[0].toUpperCase()}${row.key.slice(1)}`;
          return (
            <div
              key={row.key}
              role="group"
              aria-label={row.label}
              className="rounded-md border border-slate-800 p-3"
            >
              <div className="flex flex-col gap-3 lg:grid lg:grid-cols-[minmax(10rem,1fr)_auto_minmax(14rem,2fr)_auto] lg:items-start lg:gap-4">
                <div>
                  <div className="text-sm font-medium text-slate-200">
                    {row.label}
                  </div>
                  <div className="mt-1 text-xs text-slate-500">
                    {simulator
                      ? "Simulator value"
                      : fixed
                        ? "Fixed override"
                        : "Read from tag"}
                  </div>
                </div>
                <SourceToggle
                  label={row.label}
                  value={values.source}
                  options={[
                    {
                      value: "tag",
                      label: "Read tag",
                      disabled: simulator,
                    },
                    { value: "fixed", label: "Fixed value" },
                  ]}
                  onChange={(source) => onValueSourceChange(row.key, source)}
                />
                <div className="min-w-0">
                  {fixed ? (
                    row.kind === "direction" ? (
                      <SelectField
                        label={`${row.label} fixed value`}
                        value={values.value as "" | ControllerDirection}
                        onChange={(value) =>
                          onValueChange(
                            valueKey as "opcDirection" | "simDirection",
                            value,
                          )
                        }
                        options={["direct", "reverse"]}
                        displayLabel={(value) => DIRECTION_LABELS[value]}
                        placeholder="Choose direction"
                        required
                        hint={
                          simulator
                            ? "Used by the simulator; not sent as an OPC DA override."
                            : "Replaces the live controller-direction tag for this tune."
                        }
                      />
                    ) : (
                      <NumberField
                        label={`${row.label} fixed value`}
                        value={values.value as NumOrBlank}
                        onChange={(value) =>
                          onValueChange(
                            valueKey as
                              | "opcPvRangeHigh"
                              | "opcPvRangeLow"
                              | "opcMvRangeHigh"
                              | "opcMvRangeLow"
                              | "simPvRangeHigh"
                              | "simPvRangeLow"
                              | "simMvRangeHigh"
                              | "simMvRangeLow",
                            value,
                          )
                        }
                        required
                        step="any"
                        hint={
                          simulator
                            ? "Used by the simulator; not sent as an OPC DA override."
                            : "Replaces the live range-tag read for this tune."
                        }
                      />
                    )
                  ) : (
                    <div>
                      <div className="text-xs uppercase tracking-wide text-slate-500">
                        Effective value
                      </div>
                      <div className="mt-1 min-h-8 rounded-md border border-slate-800 bg-slate-950 px-3 py-1.5">
                        {displayTag(
                          values.preview,
                          template
                            ? "Not used by this template"
                            : "Choose a template",
                        )}
                      </div>
                    </div>
                  )}
                  {fixed && (
                    <div className="mt-1 text-xs text-slate-500">
                      Current value: {valueText(values.value)}
                    </div>
                  )}
                </div>
                <Button
                  onClick={() => onResetValue(row.key)}
                  disabled={simulator || !fixed}
                  title={
                    simulator
                      ? "Simulator values are independent of OPC mapping overrides."
                      : !fixed
                        ? "This row already reads its value from the OPC tag."
                        : "Reset this row to read its value from the OPC tag."
                  }
                >
                  Reset
                </Button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
