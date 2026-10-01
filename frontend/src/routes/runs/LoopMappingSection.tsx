import { FormSection } from "../../components/ui";
import { LoopMappingEditor } from "./LoopMappingEditor";
import type { NewRunSectionProps } from "./newRunSectionShared";

export function LoopMappingSection({
  form,
  template,
  onTagSourceChange,
  onTagChange,
  onValueSourceChange,
  onValueTagChange,
  onValueChange,
  onResetTag,
  onResetValue,
  onResetAll,
  fieldError,
}: Pick<
  NewRunSectionProps,
  | "form"
  | "template"
  | "onTagSourceChange"
  | "onTagChange"
  | "onValueSourceChange"
  | "onValueTagChange"
  | "onValueChange"
  | "onResetTag"
  | "onResetValue"
  | "onResetAll"
  | "fieldError"
>) {
  return (
    <FormSection
      title="Loop mapping"
      collapsible
      defaultOpen
      documentationId="new-tune.loop-mapping"
    >
      <LoopMappingEditor
        state={form}
        template={template}
        onTagSourceChange={onTagSourceChange}
        onTagChange={onTagChange}
        onValueSourceChange={onValueSourceChange}
        onValueTagChange={onValueTagChange}
        onValueChange={onValueChange}
        onResetTag={onResetTag}
        onResetValue={onResetValue}
        onResetAll={onResetAll}
        fieldError={fieldError}
      />
    </FormSection>
  );
}
