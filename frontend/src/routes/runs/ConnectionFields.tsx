import { OpcServerDiscovery } from "../../components/OpcServerDiscovery";
import {
  Button,
  FormSection,
  SelectField,
  TextAreaField,
  TextField,
} from "../../components/ui";
import { DRIVER_LABELS } from "../../lib/enumLabels";
import { DRIVERS } from "./newRunFormState";
import {
  disabledGatewayHint,
  tagNameHint,
  templateHint,
  type NewRunSectionProps,
} from "./newRunSectionShared";

export function ConnectionFields({
  form,
  templates,
  templatesPending,
  onChange,
  onTagNameChange,
  onDriverChange,
  onTemplateChange,
  onOpenTagBrowser,
}: Pick<
  NewRunSectionProps,
  | "form"
  | "templates"
  | "templatesPending"
  | "onChange"
  | "onTagNameChange"
  | "onDriverChange"
  | "onTemplateChange"
  | "onOpenTagBrowser"
>) {
  const isSimulator = form.driver === "simulator";
  const hasServer = form.server.trim().length > 0;

  return (
    <FormSection
      title="Connection"
      collapsible
      defaultOpen
      documentationId="new-tune.connection"
    >
      <SelectField
        label="Driver"
        value={form.driver}
        onChange={onDriverChange}
        options={DRIVERS}
        displayLabel={(value) => DRIVER_LABELS[value]}
      />
      <div>
        <SelectField
          label="Template"
          value={form.template}
          onChange={onTemplateChange}
          options={(templates ?? []).map((template) => template.name)}
          placeholder={
            templatesPending ? "Loading templates…" : "Choose a template"
          }
        />
        <span className="mt-1 block text-xs text-slate-500">
          {templateHint(form.driver)}
        </span>
      </div>
      <TextField
        label="Bridge host"
        disabled={isSimulator}
        value={form.bridgeHost}
        onChange={(value) => onChange("bridgeHost", value)}
        placeholder="Defaults to this server's own configured bridge host"
        hint={disabledGatewayHint(form.driver)}
      />
      <div>
        <TextField
          label="OPC DA server ProgID"
          required={!isSimulator}
          disabled={isSimulator}
          value={form.server}
          onChange={(value) => onChange("server", value)}
          placeholder="e.g. Matrikon.OPC.Simulation"
          hint={disabledGatewayHint(form.driver)}
        />
        {form.driver === "opcda" && (
          <OpcServerDiscovery
            bridgeHost={form.bridgeHost}
            onSelect={(value) => onChange("server", value)}
          />
        )}
      </div>
      <div>
        <TextField
          label="Tag name"
          required={!isSimulator}
          disabled={isSimulator}
          value={form.tagname}
          onChange={onTagNameChange}
          hint={tagNameHint(form.driver)}
        />
        {form.driver === "opcda" && (
          <div className="mt-1">
            <Button
              onClick={onOpenTagBrowser}
              disabled={!hasServer}
              title={
                hasServer ? undefined : "Enter an OPC DA server ProgID first."
              }
            >
              Browse tags
            </Button>
          </div>
        )}
      </div>
      <TextAreaField
        label="Notes"
        value={form.notes}
        onChange={(value) => onChange("notes", value)}
        full
        placeholder="Optional context, observations, or follow-up actions"
        hint="Notes can be edited or cleared from the tune history."
      />
    </FormSection>
  );
}
