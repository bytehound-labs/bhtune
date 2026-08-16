import { Link, useNavigate, useParams } from "react-router";
import { useDeleteTemplate, useTemplate } from "../../api/templates";
import {
  Badge,
  Button,
  ErrorBanner,
  Field,
  LoadingState,
  PageHeading,
  Section,
} from "../../components/ui";

const originTone = {
  builtin: "success",
  catalog: "neutral",
  user: "warning",
} as const;

function yesNo(value: boolean) {
  return value ? "Yes" : "No";
}

function orDash(value: string | null | undefined) {
  return value && value.length > 0 ? value : "—";
}

export function TemplateDetailPage() {
  const { name = "" } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const template = useTemplate(name);
  const deleteTemplate = useDeleteTemplate();
  const isUserOwned = template.isSuccess && template.data.origin === "user";

  return (
    <div>
      <PageHeading
        title={name}
        description={
          isUserOwned
            ? "User-owned template — editable."
            : "Re-seeded from its source file on every startup — not editable here."
        }
        actions={
          <>
            <Link to="/templates">
              <Button>Back to list</Button>
            </Link>
            {isUserOwned && (
              <Link to={`/templates/${encodeURIComponent(name)}/edit`}>
                <Button>Edit</Button>
              </Link>
            )}
            <Button
              variant="danger"
              disabled={deleteTemplate.isPending}
              onClick={() => {
                if (
                  window.confirm(
                    `Delete template "${name}"? This cannot be undone.`,
                  )
                ) {
                  deleteTemplate.mutate(name, {
                    onSuccess: () => navigate("/templates"),
                  });
                }
              }}
            >
              Delete
            </Button>
          </>
        }
      />

      {template.isPending && <LoadingState message="Loading template…" />}
      {template.isError && <ErrorBanner message={template.error.message} />}
      {deleteTemplate.isError && (
        <div className="mb-4">
          <ErrorBanner message={deleteTemplate.error.message} />
        </div>
      )}

      {template.isSuccess && (
        <>
          <Section title="Identity">
            <Field label="Name" value={template.data.name} />
            <Field
              label="Origin"
              value={
                <Badge tone={originTone[template.data.origin]}>
                  {template.data.origin}
                </Badge>
              }
            />
            <Field
              label="Versions"
              value={
                template.data.versions && template.data.versions.length > 0
                  ? template.data.versions.join(", ")
                  : "—"
              }
            />
            <Field label="Source" value={orDash(template.data.source)} full />
            <Field
              label="Description"
              value={orDash(template.data.description)}
              full
            />
            <Field
              label="Created"
              value={new Date(template.data.created_at).toLocaleString()}
            />
            <Field
              label="Updated"
              value={new Date(template.data.updated_at).toLocaleString()}
            />
          </Section>

          <Section title="Behavior">
            <Field
              label="Revert mode after test"
              value={yesNo(template.data.revert_mode)}
            />
            <Field
              label="Proportional type"
              value={template.data.proportional_type}
            />
            <Field
              label="Integral type"
              value={`${template.data.integral_type} (${template.data.integral_unit})`}
            />
            <Field
              label="Derivative type"
              value={`${template.data.derivative_type} (${template.data.derivative_unit})`}
            />
          </Section>

          <Section title="Tag suffixes">
            <Field
              label="Process variable"
              value={template.data.process_variable_suffix}
            />
            <Field
              label="Manipulated variable"
              value={template.data.manipulated_variable_suffix}
            />
            <Field
              label="Setpoint"
              value={template.data.setpoint_variable_suffix}
            />
            <Field
              label="Controller direction"
              value={template.data.controller_direction_suffix}
            />
            <Field
              label="Controller mode"
              value={template.data.controller_mode_suffix}
            />
            <Field
              label="Mode attribute"
              value={orDash(template.data.mode_attribute_suffix)}
            />
            <Field
              label="Upper PV range"
              value={template.data.upper_pv_range_suffix}
            />
            <Field
              label="Lower PV range"
              value={template.data.lower_pv_range_suffix}
            />
            <Field
              label="Upper MV range"
              value={template.data.upper_mv_range_suffix}
            />
            <Field
              label="Lower MV range"
              value={template.data.lower_mv_range_suffix}
            />
            <Field
              label="Proportional constant"
              value={template.data.proportional_constant_suffix}
            />
            <Field
              label="Integral constant"
              value={template.data.integral_constant_suffix}
            />
            <Field
              label="Derivative constant"
              value={template.data.derivative_constant_suffix}
            />
          </Section>

          <Section title="Mode values">
            <Field
              label="Manual value"
              value={template.data.mode_manual_value}
            />
            <Field label="Auto value" value={template.data.mode_auto_value} />
            <Field
              label="Mode-attribute Program value"
              value={orDash(template.data.mode_attribute_program_value)}
            />
            <Field
              label="Controller-direction Direct value"
              value={template.data.controller_action_direct_value}
            />
          </Section>
        </>
      )}
    </div>
  );
}
