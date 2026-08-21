import { useState } from "react";
import { useNavigate, Link } from "react-router";
import { useCreateTemplate } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import { Button, ErrorBanner, PageHeading } from "../../components/ui";
import { TemplateFormFields } from "./TemplateFormFields";
import {
  blankTemplateForm,
  templateFormStateToTemplate,
  type TemplateFormState,
} from "./templateFormState";

export function TemplateCreatePage() {
  const navigate = useNavigate();
  const createTemplate = useCreateTemplate();
  const [form, setForm] = useState<TemplateFormState>(blankTemplateForm);

  function set<K extends keyof TemplateFormState>(
    key: K,
    value: TemplateFormState[K],
  ) {
    setForm((prev) => ({ ...prev, [key]: value }));
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const template = templateFormStateToTemplate(form);
    createTemplate.mutate(template, {
      onSuccess: () =>
        navigate(`/templates/${encodeURIComponent(template.name)}`),
    });
  }

  return (
    <div>
      <PageHeading
        title="New template"
        description="Creates a user-owned template."
        actions={
          <Link to="/templates">
            <Button>Cancel</Button>
          </Link>
        }
      />

      {createTemplate.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              createTemplate.error,
              "Unable to create the template.",
            )}
          />
        </div>
      )}

      <form onSubmit={handleSubmit}>
        <TemplateFormFields form={form} set={set} />

        <div className="flex gap-2">
          <Button
            type="submit"
            variant="primary"
            disabled={createTemplate.isPending}
          >
            {createTemplate.isPending ? "Creating…" : "Create template"}
          </Button>
          <Link to="/templates">
            <Button>Cancel</Button>
          </Link>
        </div>
      </form>
    </div>
  );
}
