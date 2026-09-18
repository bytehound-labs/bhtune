import { useState } from "react";
import { Link } from "react-router";
import { useDeleteTemplate, useTemplates } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import {
  Badge,
  Button,
  ConfirmModal,
  EmptyState,
  ErrorBanner,
  LoadingState,
  PageHeading,
} from "../../components/ui";

const originTone = {
  builtin: "success",
  catalog: "neutral",
  user: "warning",
} as const;

export function TemplateListPage() {
  const templates = useTemplates();
  const deleteTemplate = useDeleteTemplate();
  const [templateToDelete, setTemplateToDelete] = useState<string | null>(null);

  function requestDelete(name: string) {
    deleteTemplate.reset();
    setTemplateToDelete(name);
  }

  function cancelDelete() {
    if (deleteTemplate.isPending) return;
    deleteTemplate.reset();
    setTemplateToDelete(null);
  }

  function confirmDelete() {
    if (!templateToDelete || deleteTemplate.isPending) return;
    deleteTemplate.mutate(templateToDelete, {
      onSuccess: () => setTemplateToDelete(null),
    });
  }

  return (
    <div>
      <PageHeading
        title="Templates"
        documentationId="templates.list-page"
        description="DCS/PLC tag-mapping presets used to derive a loop's OPC tag set from a single process-variable tag."
        actions={
          <Link to="/templates/new">
            <Button variant="primary">New template</Button>
          </Link>
        }
      />

      {templates.isPending && <LoadingState message="Loading templates…" />}
      {templates.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            templates.error,
            "Unable to load templates.",
          )}
        />
      )}
      {templates.isSuccess && templates.data.length === 0 && (
        <EmptyState message="No templates yet. Create one to get started." />
      )}

      {templates.isSuccess && templates.data.length > 0 && (
        <div className="overflow-hidden rounded-lg border border-slate-800">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
              <tr>
                <th className="px-4 py-2 font-medium">Name</th>
                <th className="px-4 py-2 font-medium">Origin</th>
                <th className="px-4 py-2 font-medium">Versions</th>
                <th className="px-4 py-2 font-medium">Description</th>
                <th className="px-4 py-2 font-medium">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {templates.data.map((template) => (
                <tr key={template.id} className="hover:bg-slate-900/30">
                  <td className="px-4 py-3 font-medium">
                    <Link
                      to={`/templates/${encodeURIComponent(template.name)}`}
                      className="hover:underline"
                    >
                      {template.name}
                    </Link>
                  </td>
                  <td className="px-4 py-3">
                    <Badge tone={originTone[template.origin]}>
                      {template.origin}
                    </Badge>
                  </td>
                  <td className="px-4 py-3 text-slate-400">
                    {template.versions && template.versions.length > 0
                      ? template.versions.join(", ")
                      : "—"}
                  </td>
                  <td className="max-w-xs truncate px-4 py-3 text-slate-400">
                    {template.description ?? "—"}
                  </td>
                  <td className="px-4 py-3 text-right">
                    <Button
                      variant="danger"
                      disabled={
                        deleteTemplate.isPending &&
                        deleteTemplate.variables === template.name
                      }
                      onClick={() => requestDelete(template.name)}
                    >
                      Delete
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {deleteTemplate.isError && !templateToDelete && (
        <div className="mt-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              deleteTemplate.error,
              "Unable to delete the template.",
            )}
          />
        </div>
      )}
      {templateToDelete && (
        <ConfirmModal
          title="Delete template?"
          onCancel={cancelDelete}
          onConfirm={confirmDelete}
          pending={deleteTemplate.isPending}
          confirmLabel="Delete template"
          pendingLabel="Deleting template…"
          errorMessage={
            deleteTemplate.isError
              ? userFacingErrorMessage(
                  deleteTemplate.error,
                  "Unable to delete the template.",
                )
              : null
          }
          documentationId="templates.delete-confirmation"
        >
          <p>
            Delete <strong>{templateToDelete}</strong>? This removes the
            template from the current database and cannot be undone here.
          </p>
          <p className="mt-2 text-slate-400">
            Built-in and catalog templates will return on the next startup while
            their source definition remains available.
          </p>
        </ConfirmModal>
      )}
    </div>
  );
}
