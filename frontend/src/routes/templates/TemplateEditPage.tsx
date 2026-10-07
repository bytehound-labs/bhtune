import { useState, type SubmitEvent } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useTemplate, useUpdateTemplate } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import {
  Button,
  ErrorBanner,
  LoadingState,
  PageHeading,
} from "../../components/ui";
import { TemplateFormFields } from "./TemplateFormFields";
import {
  blankTemplateForm,
  pidRoundingFormError,
  templateFormStateToTemplate,
  templateToFormState,
  type TemplateFormState,
} from "./templateFormState";

export function TemplateEditPage() {
  const { name = "" } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const template = useTemplate(name);
  const updateTemplate = useUpdateTemplate();
  const [editorState, setEditorState] = useState(() => ({
    template: template.data,
    form: template.data
      ? templateToFormState(template.data)
      : blankTemplateForm,
  }));
  if (template.data && template.data !== editorState.template) {
    setEditorState({
      template: template.data,
      form: templateToFormState(template.data),
    });
  }
  const form = editorState.form;
  const precisionError = pidRoundingFormError(form.pid_rounding);

  function set<K extends keyof TemplateFormState>(
    key: K,
    value: TemplateFormState[K],
  ) {
    setEditorState((previous) => ({
      ...previous,
      form: { ...previous.form, [key]: value },
    }));
  }

  function handleSubmit(e: SubmitEvent<HTMLFormElement>) {
    e.preventDefault();
    if (precisionError) return;
    const updated = templateFormStateToTemplate(form);
    updateTemplate.mutate(
      { name, template: updated },
      {
        onSuccess: () => navigate(`/templates/${encodeURIComponent(name)}`),
      },
    );
  }

  const isNotUserOwned = template.isSuccess && template.data.origin !== "user";

  return (
    <div>
      <PageHeading
        title={`Edit ${name}`}
        documentationId="templates.edit-page"
        description="Renaming isn't supported here — delete and recreate the template instead."
        actions={
          <Link to={`/templates/${encodeURIComponent(name)}`}>
            <Button>Cancel</Button>
          </Link>
        }
      />

      {template.isPending && <LoadingState message="Loading template…" />}
      {template.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            template.error,
            "Unable to load the template.",
          )}
        />
      )}
      {isNotUserOwned && (
        <div className="mb-4">
          <ErrorBanner message="Built-in and catalog templates are managed by BHTune and cannot be edited here." />
        </div>
      )}
      {updateTemplate.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              updateTemplate.error,
              "Unable to save the template.",
            )}
          />
        </div>
      )}

      {template.isSuccess && (
        <form onSubmit={handleSubmit}>
          <TemplateFormFields form={form} set={set} nameEditable={false} />

          <div className="flex gap-2">
            <Button
              type="submit"
              variant="primary"
              disabled={
                updateTemplate.isPending ||
                isNotUserOwned ||
                Boolean(precisionError)
              }
            >
              {updateTemplate.isPending ? "Saving…" : "Save changes"}
            </Button>
            <Link to={`/templates/${encodeURIComponent(name)}`}>
              <Button>Cancel</Button>
            </Link>
          </div>
        </form>
      )}
    </div>
  );
}
