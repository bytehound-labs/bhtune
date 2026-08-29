import { useEffect, useRef, useState, type SubmitEvent } from "react";
import { Link, useLocation, useNavigate } from "react-router";
import {
  useLastRunRequest,
  useRunDraft,
  useSaveRunDraft,
  useStartRun,
} from "../../api/runs";
import type { StartRunRequest } from "../../api/runs";
import { useTemplates } from "../../api/templates";
import { userFacingErrorMessage } from "../../api/errors";
import { OpcTagBrowserModal } from "../../components/OpcTagBrowserModal";
import { Button, ErrorBanner, PageHeading } from "../../components/ui";
import { replaceTagSuffix } from "../../lib/opcTags";
import {
  DEFAULT_TAG_MAPPING_SOURCES,
  DEFAULT_VALUE_MAPPING_SOURCES,
  EMPTY_TAG_OVERRIDES,
  EMPTY_VALUE_TAG_OVERRIDES,
  type ControllerDirection,
  type NumOrBlank,
  type TagMappingSource,
  type TagOverrideKey,
  type ValueMappingKey,
  type ValueMappingSource,
} from "./mappingState";
import { NewRunForm } from "./NewRunForm";
import {
  buildRequest,
  draftFromForm,
  formFromDraft,
  formFromRequest,
  initialForm,
  templateTagFor,
  templateValueTagFor,
  type FormState,
  type ProcessType,
  type TuneDriver,
  TEMPERATURE_PROCESS_TYPES,
} from "./newRunFormState";

type DuplicateRunLocationState = {
  readonly duplicateRequest: StartRunRequest;
  readonly duplicateFromRunId: number;
};

export interface DuplicateRunState extends DuplicateRunLocationState {}

type MappingValueKey =
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

const VALUE_FORM_KEYS: Record<ValueMappingKey, MappingValueKey> = {
  direction: "opcDirection",
  pvRangeHigh: "opcPvRangeHigh",
  pvRangeLow: "opcPvRangeLow",
  mvRangeHigh: "opcMvRangeHigh",
  mvRangeLow: "opcMvRangeLow",
};

type PrefillSource =
  | { readonly kind: "duplicate"; readonly runId: number }
  | { readonly kind: "draft" }
  | { readonly kind: "last-run" };

function prefillMessage(source: PrefillSource): string {
  switch (source.kind) {
    case "duplicate":
      return `Loaded settings from tune #${source.runId}.`;
    case "draft":
      return "Loaded your saved Tune draft.";
    case "last-run":
      return "Loaded settings from the most recent tune.";
  }
}

function isDuplicateRunState(
  state: unknown,
): state is DuplicateRunLocationState {
  if (typeof state !== "object" || state === null) return false;
  const candidate = state as Record<string, unknown>;
  return (
    typeof candidate.duplicateFromRunId === "number" &&
    typeof candidate.duplicateRequest === "object" &&
    candidate.duplicateRequest !== null
  );
}

export function NewRunPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const templates = useTemplates();
  const startRun = useStartRun();
  const lastRunRequest = useLastRunRequest();
  const runDraft = useRunDraft();
  const saveRunDraft = useSaveRunDraft();
  const duplicateState = isDuplicateRunState(location.state)
    ? location.state
    : undefined;

  const [form, setForm] = useState<FormState>(() =>
    duplicateState
      ? formFromRequest(duplicateState.duplicateRequest)
      : initialForm,
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  const [prefillSource, setPrefillSource] = useState<PrefillSource | null>(
    () =>
      duplicateState
        ? { kind: "duplicate", runId: duplicateState.duplicateFromRunId }
        : null,
  );
  const [hydrated, setHydrated] = useState(Boolean(duplicateState));
  const hydratedRef = useRef(Boolean(duplicateState));
  const preserveBlankTemplateRef = useRef(false);
  const [draftLoadError, setDraftLoadError] = useState<string | null>(null);
  const [draftSaveError, setDraftSaveError] = useState<string | null>(null);
  const draftSaveChainRef = useRef(Promise.resolve());
  const saveDraftAsync = saveRunDraft.mutateAsync;
  const [tagBrowserOpen, setTagBrowserOpen] = useState(false);
  const activeTemplate = templates.data?.find(
    (template) => template.name === form.template,
  );

  useEffect(() => {
    if (hydratedRef.current || duplicateState || runDraft.isPending) return;
    if (runDraft.data === undefined && !runDraft.isError) return;
    if (
      (runDraft.data === null || runDraft.isError) &&
      lastRunRequest.isPending
    ) {
      return;
    }

    hydratedRef.current = true;
    setHydrated(true);
    if (runDraft.isError) {
      setDraftLoadError(
        userFacingErrorMessage(
          runDraft.error,
          "Unable to load the saved Tune draft; using the available fallback.",
        ),
      );
    }
    if (runDraft.data) {
      preserveBlankTemplateRef.current = runDraft.data.template === null;
      setForm(formFromDraft(runDraft.data));
      setPrefillSource({ kind: "draft" });
    } else if (lastRunRequest.data) {
      preserveBlankTemplateRef.current = false;
      setForm(formFromRequest(lastRunRequest.data));
      setPrefillSource({ kind: "last-run" });
    } else {
      preserveBlankTemplateRef.current = false;
    }
  }, [
    duplicateState,
    lastRunRequest.data,
    lastRunRequest.isPending,
    runDraft.data,
    runDraft.error,
    runDraft.isError,
    runDraft.isPending,
  ]);

  useEffect(() => {
    if (!hydrated) return;
    const payload = draftFromForm(form);
    const timer = window.setTimeout(() => {
      draftSaveChainRef.current = draftSaveChainRef.current
        .catch(() => undefined)
        .then(() => saveDraftAsync(payload))
        .then(() => setDraftSaveError(null))
        .catch((error: unknown) => {
          setDraftSaveError(
            userFacingErrorMessage(
              error,
              "Unable to save the Tune draft. Changes will remain in this page until the server is available.",
            ),
          );
        });
    }, 400);
    return () => window.clearTimeout(timer);
  }, [form, hydrated, saveDraftAsync]);

  useEffect(() => {
    if (duplicateState || !hydrated || preserveBlankTemplateRef.current) {
      return;
    }
    setForm((previous) => {
      if (previous.template || !templates.data || templates.data.length === 0) {
        return previous;
      }
      return { ...previous, template: templates.data[0].name };
    });
  }, [duplicateState, form.template, hydrated, templates.data]);

  function resetToDefaults() {
    setPrefillSource(null);
    preserveBlankTemplateRef.current = false;
    setForm(initialForm);
  }

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((previous) => ({ ...previous, [key]: value }));
  }

  function setTagSource(key: TagOverrideKey, source: TagMappingSource) {
    setForm((previous) => {
      const template = templates.data?.find(
        (item) => item.name === previous.template,
      );
      const value =
        source === "custom" && !previous.tagOverrides[key].trim()
          ? templateTagFor(template, key, previous.tagname)
          : previous.tagOverrides[key];
      return {
        ...previous,
        tagSources: { ...previous.tagSources, [key]: source },
        tagOverrides: { ...previous.tagOverrides, [key]: value },
      };
    });
  }

  function setTagValue(key: TagOverrideKey, value: string) {
    setForm((previous) => ({
      ...previous,
      tagOverrides: { ...previous.tagOverrides, [key]: value },
    }));
  }

  function setValueSource(key: ValueMappingKey, source: ValueMappingSource) {
    setForm((previous) => {
      if (previous.driver === "simulator") return previous;
      const template = templates.data?.find(
        (item) => item.name === previous.template,
      );
      const customTag =
        source === "custom" && !previous.valueTagOverrides[key].trim()
          ? templateValueTagFor(template, key, previous.tagname)
          : previous.valueTagOverrides[key];
      return {
        ...previous,
        valueSources: { ...previous.valueSources, [key]: source },
        valueTagOverrides: {
          ...previous.valueTagOverrides,
          [key]: customTag,
        },
      };
    });
  }

  function setValueTag(key: ValueMappingKey, value: string) {
    setForm((previous) => ({
      ...previous,
      valueTagOverrides: { ...previous.valueTagOverrides, [key]: value },
    }));
  }

  function setMappingValue(
    key: MappingValueKey,
    value: NumOrBlank | ControllerDirection,
  ) {
    setForm((previous) => ({ ...previous, [key]: value }));
  }

  function resetTag(key: TagOverrideKey) {
    setForm((previous) => ({
      ...previous,
      tagSources: { ...previous.tagSources, [key]: "template" },
      tagOverrides: { ...previous.tagOverrides, [key]: "" },
    }));
  }

  function resetValue(key: ValueMappingKey) {
    setForm((previous) => {
      if (previous.driver === "simulator") return previous;
      const valueKey = VALUE_FORM_KEYS[key];
      return {
        ...previous,
        valueSources: { ...previous.valueSources, [key]: "tag" },
        valueTagOverrides: { ...previous.valueTagOverrides, [key]: "" },
        [valueKey]: "",
      };
    });
  }

  function resetMapping() {
    setForm((previous) => ({
      ...previous,
      tagSources: { ...DEFAULT_TAG_MAPPING_SOURCES },
      valueSources: { ...DEFAULT_VALUE_MAPPING_SOURCES },
      tagOverrides: { ...EMPTY_TAG_OVERRIDES },
      valueTagOverrides: { ...EMPTY_VALUE_TAG_OVERRIDES },
      opcDirection: "",
      opcPvRangeHigh: "",
      opcPvRangeLow: "",
      opcMvRangeHigh: "",
      opcMvRangeLow: "",
    }));
  }

  function setDriver(value: TuneDriver) {
    setForm((previous) => {
      if (value !== "simulator") return { ...previous, driver: value };
      return {
        ...previous,
        driver: value,
        simPvRangeHigh:
          previous.simPvRangeHigh === "" ? 100 : previous.simPvRangeHigh,
        simPvRangeLow:
          previous.simPvRangeLow === "" ? 0 : previous.simPvRangeLow,
        simMvRangeHigh:
          previous.simMvRangeHigh === "" ? 100 : previous.simMvRangeHigh,
        simMvRangeLow:
          previous.simMvRangeLow === "" ? 0 : previous.simMvRangeLow,
        simDirection:
          previous.simDirection === "" ? "reverse" : previous.simDirection,
      };
    });
  }

  function setTemplate(value: string) {
    setForm((previous) => {
      const template = templates.data?.find((item) => item.name === value);
      const tagname = template
        ? replaceTagSuffix(previous.tagname, template.process_variable_suffix)
        : previous.tagname;
      return { ...previous, template: value, tagname };
    });
  }

  function setProcessType(value: ProcessType) {
    setForm((previous) => ({
      ...previous,
      processType: value,
      controllerType:
        previous.controllerType === "pid" &&
        !TEMPERATURE_PROCESS_TYPES.has(value)
          ? "pi"
          : previous.controllerType,
    }));
  }

  function submitTune() {
    setValidationError(null);
    const request = buildRequest(form);
    if (typeof request === "string") {
      setValidationError(request);
      return;
    }
    startRun.mutate(request, {
      onSuccess: (data) => navigate(`/runs/${data.id}`),
    });
  }

  function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    submitTune();
  }

  return (
    <div>
      <PageHeading
        title="New tune"
        description="Configure and start a tune."
        actions={
          <>
            <Button
              variant="primary"
              disabled={startRun.isPending}
              onClick={submitTune}
            >
              {startRun.isPending ? "Starting…" : "Start tune"}
            </Button>
            <Link to="/runs">
              <Button>Cancel</Button>
            </Link>
            <Button onClick={resetToDefaults}>Reset to defaults</Button>
          </>
        }
      />

      {prefillSource !== null && (
        <div className="mb-4 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm text-slate-300">
          {prefillMessage(prefillSource)} Change anything below, or "Reset to
          defaults" to return to the built-in defaults.
        </div>
      )}

      {(draftLoadError || draftSaveError) && (
        <div className="mb-4 space-y-2">
          {draftLoadError && <ErrorBanner message={draftLoadError} />}
          {draftSaveError && <ErrorBanner message={draftSaveError} />}
        </div>
      )}

      {validationError && (
        <div className="mb-4">
          <ErrorBanner message={validationError} />
        </div>
      )}
      {startRun.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              startRun.error,
              "Unable to start the tune.",
            )}
          />
        </div>
      )}
      {templates.isError && (
        <div className="mb-4">
          <ErrorBanner
            message={userFacingErrorMessage(
              templates.error,
              "Unable to load templates.",
            )}
          />
        </div>
      )}

      <NewRunForm
        form={form}
        template={activeTemplate}
        templates={templates.data}
        templatesPending={templates.isPending}
        onSubmit={handleSubmit}
        onChange={set}
        onDriverChange={setDriver}
        onTemplateChange={setTemplate}
        onProcessTypeChange={setProcessType}
        onTagSourceChange={setTagSource}
        onTagChange={setTagValue}
        onValueSourceChange={setValueSource}
        onValueTagChange={setValueTag}
        onValueChange={setMappingValue}
        onResetTag={resetTag}
        onResetValue={resetValue}
        onResetAll={resetMapping}
        onOpenTagBrowser={() => setTagBrowserOpen(true)}
      />

      {tagBrowserOpen && (
        <OpcTagBrowserModal
          bridgeHost={form.bridgeHost}
          opcServer={form.server}
          template={activeTemplate}
          initialTag={form.tagname}
          onClose={() => setTagBrowserOpen(false)}
          onSelect={(tag) => set("tagname", tag)}
        />
      )}
    </div>
  );
}
