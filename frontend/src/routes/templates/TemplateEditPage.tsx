import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useTemplate, useUpdateTemplate } from "../../api/templates";
import {
  Button,
  ErrorBanner,
  LoadingState,
  PageHeading,
} from "../../components/ui";
import { TemplateFormFields } from "./TemplateFormFields";
import {
  blankTemplateForm,
  templateFormStateToTemplate,
  templateToFormState,
  type TemplateFormState,
} from "./templateFormState";

export function TemplateEditPage() {
  const { name = "" } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const template = useTemplate(name);
  const updateTemplate = useUpdateTemplate();
  const [form, setForm] = useState<TemplateFormState>(blankTemplateForm);

  // Populate the form once the existing template loads. Only runs again if the loaded
  // template itself changes (e.g. a refetch after an unrelated tab edited it) -- not on
  // every render, which would otherwise stomp on in-progress edits.
  useEffect(() => {
    if (template.data) {
      setForm(templateToFormState(template.data));
    }
  }, [template.data]);

  function set<K extends keyof TemplateFormState>(
    key: K,
    value: TemplateFormState[K],
  ) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
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
        description="Renaming isn't supported here — delete and recreate the template instead."
        actions={
          <Link to={`/templates/${encodeURIComponent(name)}`}>
            <Button>Cancel</Button>
          </Link>
        }
      />

      {template.isPending && <LoadingState message="Loading template…" />}
      {template.isError && <ErrorBanner message={template.error.message} />}
      {isNotUserOwned && (
        <div className="mb-4">
          <ErrorBanner
            message={`This template's origin is '${template.data?.origin}', not 'user' -- it's re-seeded from its source file on every startup, so it can't be edited here. Saving will fail with a 409.`}
          />
        </div>
      )}
      {updateTemplate.isError && (
        <div className="mb-4">
          <ErrorBanner message={updateTemplate.error.message} />
        </div>
      )}

      {template.isSuccess && (
        <form onSubmit={handleSubmit}>
          <TemplateFormFields form={form} set={set} nameEditable={false} />

          <div className="flex gap-2">
            <Button
              type="submit"
              variant="primary"
              disabled={updateTemplate.isPending || isNotUserOwned}
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
