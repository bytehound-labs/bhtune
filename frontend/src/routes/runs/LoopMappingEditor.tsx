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
  type ValueTagOverrideFormState,
} from "./mappingState";

type TemplateResponse = components["schemas"]["TemplateResponse"];

type TagRow = {
  key: TagOverrideKey;
  label: string;
  previewLabel: string;
  description: string;
};

const TAG_ROWS: readonly TagRow[] = [
  {
    key: "processVariable",
    label: "Process variable (PV)",
    previewLabel: "Process variable (PV)",
    description:
      "Measured process value read and evaluated during the relay test.",
  },
  {
    key: "manipulatedVariable",
    label: "Manipulated variable (MV)",
    previewLabel: "Manipulated variable (MV)",
    description:
      "Manipulated output switched up and down during the relay test.",
  },
  {
    key: "setpointVariable",
    label: "Setpoint",
    previewLabel: "Setpoint",
    description:
      "Target captured before the test and restored when the loop starts in Auto.",
  },
  {
    key: "controllerMode",
    label: "Controller mode",
    previewLabel: "Controller mode",
    description:
      "Places the loop in Manual for the test, then restores its original mode.",
  },
  {
    key: "modeAttribute",
    label: "Mode attribute",
    previewLabel: "Mode attribute",
    description:
      "Places the controller in the required program/computer mode, then restores it.",
  },
  {
    key: "proportionalConstant",
    label: "Proportional constant",
    previewLabel: "Proportional constant",
    description:
      "PID proportional setting read for write-back and revert operations.",
  },
  {
    key: "integralConstant",
    label: "Integral constant",
    previewLabel: "Integral constant",
    description:
      "PID integral setting read for write-back and revert operations.",
  },
  {
    key: "derivativeConstant",
    label: "Derivative constant",
    previewLabel: "Derivative constant",
    description:
      "PID derivative setting read for write-back and revert operations.",
  },
];

type ValueRow = {
  key: ValueMappingKey;
  label: string;
  previewLabel: string;
  kind: "direction" | "number";
  description: string;
};

const VALUE_ROWS: readonly ValueRow[] = [
  {
    key: "direction",
    label: "Controller direction",
    previewLabel: "Controller direction",
    kind: "direction",
    description:
      "Tells the tuning math whether increasing MV raises or lowers PV.",
  },
  {
    key: "pvRangeHigh",
    label: "PV range high",
    previewLabel: "PV range high",
    kind: "number",
    description:
      "Engineering bound used to validate, normalize, and calculate relay amplitude.",
  },
  {
    key: "pvRangeLow",
    label: "PV range low",
    previewLabel: "PV range low",
    kind: "number",
    description:
      "Engineering bound used to validate, normalize, and calculate relay amplitude.",
  },
  {
    key: "mvRangeHigh",
    label: "MV range high",
    previewLabel: "MV range high",
    kind: "number",
    description:
      "Engineering bound used to validate, normalize, and calculate relay amplitude.",
  },
  {
    key: "mvRangeLow",
    label: "MV range low",
    previewLabel: "MV range low",
    kind: "number",
    description:
      "Engineering bound used to validate, normalize, and calculate relay amplitude.",
  },
];

type MappingValueState = {
  driver: TuneDriver;
  tagname: string;
  tagOverrides: TagOverrideFormState;
  valueTagOverrides: ValueTagOverrideFormState;
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

type ValueChangeKey =
  | "opcDirection"
  | "opcPvRangeHigh"
  | "opcPvRangeLow"
  | "opcMvRangeHigh"
  | "opcMvRangeLow"
  | "simDirection"
  | "simPvRangeHigh"
  | "simPvRangeLow"
  | "simMvRangeHigh"
  | "simMvRangeLow";

type Props = {
  readonly state: MappingValueState;
  readonly template: TemplateResponse | undefined;
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
    key: ValueChangeKey,
    value: NumOrBlank | ControllerDirection,
  ) => void;
  readonly onResetTag: (key: TagOverrideKey) => void;
  readonly onResetValue: (key: ValueMappingKey) => void;
  readonly onResetAll: () => void;
};

type SourceToggleProps<T extends string> = {
  readonly label: string;
  readonly value: T;
  readonly options: readonly {
    readonly value: T;
    readonly label: string;
    readonly disabled?: boolean;
  }[];
  readonly onChange: (value: T) => void;
  readonly disabled?: boolean;
};

function SourceToggle<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled = false,
}: SourceToggleProps<T>) {
  return (
    <fieldset className="inline-flex rounded-md border border-slate-700">
      <legend className="sr-only">{label} source</legend>
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
    </fieldset>
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

function templateTagPreview(
  tagname: string,
  template: TemplateResponse | undefined,
  label: string,
) {
  if (!template) return null;
  return (
    derivedTagPreview(tagname, template).find((item) => item.label === label)
      ?.tag ?? null
  );
}

function tagValue(
  row: TagRow,
  state: MappingValueState,
  template: TemplateResponse | undefined,
) {
  const preview = templateTagPreview(state.tagname, template, row.previewLabel);
  const source = state.tagSources[row.key];
  const custom = state.tagOverrides[row.key].trim();
  return {
    preview,
    source,
    custom,
    effective: source === "custom" && custom ? custom : (preview ?? null),
  };
}

type ValueFieldKey =
  | "opcDirection"
  | "opcPvRangeHigh"
  | "opcPvRangeLow"
  | "opcMvRangeHigh"
  | "opcMvRangeLow"
  | "simDirection"
  | "simPvRangeHigh"
  | "simPvRangeLow"
  | "simMvRangeHigh"
  | "simMvRangeLow";

const OPC_VALUE_FIELDS: Record<ValueMappingKey, ValueFieldKey> = {
  direction: "opcDirection",
  pvRangeHigh: "opcPvRangeHigh",
  pvRangeLow: "opcPvRangeLow",
  mvRangeHigh: "opcMvRangeHigh",
  mvRangeLow: "opcMvRangeLow",
};

const SIMULATOR_VALUE_FIELDS: Record<ValueMappingKey, ValueFieldKey> = {
  direction: "simDirection",
  pvRangeHigh: "simPvRangeHigh",
  pvRangeLow: "simPvRangeLow",
  mvRangeHigh: "simMvRangeHigh",
  mvRangeLow: "simMvRangeLow",
};

const OPC_VALUE_CHANGE_KEYS: Record<ValueMappingKey, ValueChangeKey> = {
  direction: "opcDirection",
  pvRangeHigh: "opcPvRangeHigh",
  pvRangeLow: "opcPvRangeLow",
  mvRangeHigh: "opcMvRangeHigh",
  mvRangeLow: "opcMvRangeLow",
};

const SIMULATOR_VALUE_CHANGE_KEYS: Record<ValueMappingKey, ValueChangeKey> = {
  direction: "simDirection",
  pvRangeHigh: "simPvRangeHigh",
  pvRangeLow: "simPvRangeLow",
  mvRangeHigh: "simMvRangeHigh",
  mvRangeLow: "simMvRangeLow",
};

function valueField(
  state: MappingValueState,
  key: ValueMappingKey,
  simulator: boolean,
) {
  const fields = simulator ? SIMULATOR_VALUE_FIELDS : OPC_VALUE_FIELDS;
  return state[fields[key]];
}

function valueState(
  row: ValueRow,
  state: MappingValueState,
  template: TemplateResponse | undefined,
) {
  const simulator = state.driver === "simulator";
  return {
    source: simulator ? "fixed" : state.valueSources[row.key],
    preview: templateTagPreview(state.tagname, template, row.previewLabel),
    value: valueField(state, row.key, simulator),
  };
}

function valueChangeKey(key: ValueMappingKey, simulator: boolean) {
  const keys = simulator ? SIMULATOR_VALUE_CHANGE_KEYS : OPC_VALUE_CHANGE_KEYS;
  return keys[key];
}

function valueText(value: NumOrBlank | "" | ControllerDirection) {
  if (value === "") return "No fixed value";
  if (value === "direct" || value === "reverse") {
    return DIRECTION_LABELS[value];
  }
  return String(value);
}

function missingTemplateMessage(template: TemplateResponse | undefined) {
  if (template) return "Not used by this template";
  return "Choose a template";
}

function EffectiveValue({
  tag,
  template,
}: {
  readonly tag: string | null;
  readonly template: TemplateResponse | undefined;
}) {
  return (
    <div>
      <div className="text-xs uppercase tracking-wide text-slate-500">
        Effective value
      </div>
      <div className="mt-1 min-h-8 cursor-not-allowed rounded-md border border-slate-700 bg-slate-800/70 px-3 py-1.5 text-slate-300">
        {displayTag(tag, missingTemplateMessage(template))}
      </div>
    </div>
  );
}

function tagSourceDescription(inactive: boolean, source: TagMappingSource) {
  if (inactive) return "Inactive for Simulator";
  if (source === "custom") return "Custom tag";
  return "Template-derived tag";
}

function tagResetTitle(inactive: boolean, source: TagMappingSource) {
  if (inactive) return "Simulator mode does not use OPC tag overrides.";
  if (source === "template")
    return "This row already uses the template-derived tag.";
  return "Reset this row to the template-derived tag.";
}

function tagValueHint(preview: string | null) {
  if (preview) return "Reset returns to the template-derived value.";
  return "This template has no default for this item; enter a site-specific tag if needed.";
}

type TagValueControlProps = {
  readonly row: TagRow;
  readonly tags: ReturnType<typeof tagValue>;
  readonly state: MappingValueState;
  readonly template: TemplateResponse | undefined;
  readonly inactive: boolean;
  readonly onTagChange: (key: TagOverrideKey, value: string) => void;
};

function TagValueControl({
  row,
  tags,
  state,
  template,
  inactive,
  onTagChange,
}: TagValueControlProps) {
  if (tags.source === "custom") {
    return (
      <TextField
        label={`${row.label} custom tag`}
        value={state.tagOverrides[row.key]}
        onChange={(value) => onTagChange(row.key, value)}
        disabled={inactive}
        placeholder={tags.preview ?? "Enter a custom OPC item ID"}
        hint={tagValueHint(tags.preview)}
      />
    );
  }

  return <EffectiveValue tag={tags.effective} template={template} />;
}

type TagMappingRowProps = {
  readonly row: TagRow;
  readonly state: MappingValueState;
  readonly template: TemplateResponse | undefined;
  readonly onTagSourceChange: (
    key: TagOverrideKey,
    source: TagMappingSource,
  ) => void;
  readonly onTagChange: (key: TagOverrideKey, value: string) => void;
  readonly onResetTag: (key: TagOverrideKey) => void;
};

function TagMappingRow({
  row,
  state,
  template,
  onTagSourceChange,
  onTagChange,
  onResetTag,
}: TagMappingRowProps) {
  const inactive = state.driver === "simulator";
  const tags = tagValue(row, state, template);

  return (
    <fieldset
      className={`m-0 rounded-md border border-slate-800 p-3 ${
        inactive ? "opacity-65" : ""
      }`}
    >
      <legend className="sr-only">{row.label}</legend>
      <div className="flex flex-col gap-3 lg:grid lg:grid-cols-[minmax(10rem,1fr)_auto_minmax(14rem,2fr)_auto] lg:items-start lg:gap-4">
        <div>
          <div className="text-sm font-medium text-slate-200">{row.label}</div>
          <div className="mt-1 text-xs text-slate-500">
            {tagSourceDescription(inactive, tags.source)}
          </div>
          <p className="mt-2 text-xs leading-relaxed text-slate-500">
            {row.description}
          </p>
        </div>
        <SourceToggle
          label={row.label}
          value={tags.source}
          options={[
            { value: "template", label: "Template tag" },
            { value: "custom", label: "Custom tag" },
          ]}
          disabled={inactive}
          onChange={(source) => onTagSourceChange(row.key, source)}
        />
        <div className="min-w-0">
          <TagValueControl
            row={row}
            tags={tags}
            state={state}
            template={template}
            inactive={inactive}
            onTagChange={onTagChange}
          />
        </div>
        <Button
          onClick={() => onResetTag(row.key)}
          disabled={inactive || tags.source === "template"}
          title={tagResetTitle(inactive, tags.source)}
        >
          Reset
        </Button>
      </div>
    </fieldset>
  );
}

function valueSourceDescription(
  simulator: boolean,
  source: ValueMappingSource,
) {
  if (simulator) return "Simulator value";
  if (source === "custom") return "Custom read tag";
  if (source === "fixed") return "Fixed value";
  return "Template-derived read tag";
}

function valueResetTitle(simulator: boolean, source: ValueMappingSource) {
  if (simulator)
    return "Simulator values are independent of OPC mapping overrides.";
  if (source === "tag")
    return "This row already reads its value from the template-derived OPC tag.";
  return "Reset this row to read its value from the OPC tag.";
}

function fixedValueHint(simulator: boolean, kind: ValueRow["kind"]) {
  if (simulator)
    return "Used by the simulator; not sent as an OPC DA override.";
  if (kind === "direction")
    return "Replaces the live controller-direction tag for this tune.";
  return "Replaces the live range-tag read for this tune.";
}

function directionValue(value: NumOrBlank | ControllerDirection) {
  if (value === "" || value === "direct" || value === "reverse") {
    return value;
  }
  return "";
}

function numberValue(value: NumOrBlank | ControllerDirection): NumOrBlank {
  if (typeof value === "number" || value === "") return value;
  return "";
}

type ValueEditorProps = {
  readonly row: ValueRow;
  readonly values: ReturnType<typeof valueState>;
  readonly state: MappingValueState;
  readonly template: TemplateResponse | undefined;
  readonly simulator: boolean;
  readonly valueKey: ValueChangeKey;
  readonly onValueTagChange: (key: ValueMappingKey, value: string) => void;
  readonly onValueChange: (
    key: ValueChangeKey,
    value: NumOrBlank | ControllerDirection,
  ) => void;
};

function ValueEditor({
  row,
  values,
  state,
  template,
  simulator,
  valueKey,
  onValueTagChange,
  onValueChange,
}: ValueEditorProps) {
  if (values.source === "custom") {
    return (
      <TextField
        label={`${row.label} custom read tag`}
        value={state.valueTagOverrides[row.key]}
        onChange={(value) => onValueTagChange(row.key, value)}
        placeholder={values.preview ?? "Enter a custom OPC item ID"}
        hint={
          values.preview
            ? "Reset returns to the template-derived read tag."
            : "This template has no default read tag; enter a site-specific tag."
        }
      />
    );
  }

  if (values.source !== "fixed") {
    return <EffectiveValue tag={values.preview} template={template} />;
  }

  if (row.kind === "direction") {
    return (
      <SelectField
        label={`${row.label} fixed value`}
        value={directionValue(values.value)}
        onChange={(value) => onValueChange(valueKey, value)}
        options={["direct", "reverse"]}
        displayLabel={(value) => DIRECTION_LABELS[value]}
        placeholder="Choose direction"
        required
        hint={fixedValueHint(simulator, row.kind)}
      />
    );
  }

  return (
    <NumberField
      label={`${row.label} fixed value`}
      value={numberValue(values.value)}
      onChange={(value) => onValueChange(valueKey, value)}
      required
      step="any"
      hint={fixedValueHint(simulator, row.kind)}
    />
  );
}

type ValueMappingRowProps = {
  readonly row: ValueRow;
  readonly state: MappingValueState;
  readonly template: TemplateResponse | undefined;
  readonly onValueSourceChange: (
    key: ValueMappingKey,
    source: ValueMappingSource,
  ) => void;
  readonly onValueTagChange: (key: ValueMappingKey, value: string) => void;
  readonly onValueChange: (
    key: ValueChangeKey,
    value: NumOrBlank | ControllerDirection,
  ) => void;
  readonly onResetValue: (key: ValueMappingKey) => void;
};

function ValueMappingRow({
  row,
  state,
  template,
  onValueSourceChange,
  onValueTagChange,
  onValueChange,
  onResetValue,
}: ValueMappingRowProps) {
  const simulator = state.driver === "simulator";
  const values = valueState(row, state, template);
  const fixed = values.source === "fixed";
  const valueKey = valueChangeKey(row.key, simulator);

  return (
    <fieldset className="m-0 rounded-md border border-slate-800 p-3">
      <legend className="sr-only">{row.label}</legend>
      <div className="flex flex-col gap-3 lg:grid lg:grid-cols-[minmax(10rem,1fr)_auto_minmax(14rem,2fr)_auto] lg:items-start lg:gap-4">
        <div>
          <div className="text-sm font-medium text-slate-200">{row.label}</div>
          <div className="mt-1 text-xs text-slate-500">
            {valueSourceDescription(simulator, values.source)}
          </div>
          <p className="mt-2 text-xs leading-relaxed text-slate-500">
            {row.description}
          </p>
        </div>
        <SourceToggle
          label={row.label}
          value={values.source}
          options={[
            {
              value: "tag",
              label: "Template tag",
              disabled: simulator,
            },
            {
              value: "custom",
              label: "Custom tag",
              disabled: simulator,
            },
            { value: "fixed", label: "Fixed value" },
          ]}
          onChange={(source) => onValueSourceChange(row.key, source)}
        />
        <div className="min-w-0">
          <ValueEditor
            row={row}
            values={values}
            state={state}
            template={template}
            simulator={simulator}
            valueKey={valueKey}
            onValueTagChange={onValueTagChange}
            onValueChange={onValueChange}
          />
          {fixed && (
            <div className="mt-1 text-xs text-slate-500">
              Current value: {valueText(values.value)}
            </div>
          )}
        </div>
        <Button
          onClick={() => onResetValue(row.key)}
          disabled={simulator || values.source === "tag"}
          title={valueResetTitle(simulator, values.source)}
        >
          Reset
        </Button>
      </div>
    </fieldset>
  );
}

export function LoopMappingEditor({
  state,
  template,
  onTagSourceChange,
  onTagChange,
  onValueSourceChange,
  onValueTagChange,
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
            Template-derived values follow the selected template and Tag name;
            custom tag and fixed values apply only to this tune.
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
        {TAG_ROWS.map((row) => (
          <TagMappingRow
            key={row.key}
            row={row}
            state={state}
            template={template}
            onTagSourceChange={onTagSourceChange}
            onTagChange={onTagChange}
            onResetTag={onResetTag}
          />
        ))}

        {VALUE_ROWS.map((row) => (
          <ValueMappingRow
            key={row.key}
            row={row}
            state={state}
            template={template}
            onValueSourceChange={onValueSourceChange}
            onValueTagChange={onValueTagChange}
            onValueChange={onValueChange}
            onResetValue={onResetValue}
          />
        ))}
      </div>
    </div>
  );
}
