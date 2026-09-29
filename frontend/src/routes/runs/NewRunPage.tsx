import {
  useEffect,
  useRef,
  useState,
  type SetStateAction,
  type SubmitEvent,
} from "react";
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
  demoControllerTypesFor,
  demoDefaultControllerTypeFor,
  demoDraftFromForm,
  demoProcessDefaultsFor,
  draftFromForm,
  formFromDemoCapabilities,
  formFromDemoDraft,
  formFromDemoDuplicate,
  formFromDraft,
  formFromRequest,
  applyTagNameChange,
  initialForm,
  processDefaultsFor,
  templateTagFor,
  templateValueTagFor,
  type FormState,
  type ProcessType,
  type TuneDriver,
  normalizeSimulatorRequest,
  TEMPERATURE_PROCESS_TYPES,
} from "./newRunFormState";
import type {
  AppCapabilities,
  SimulatorCapabilities,
} from "../../api/capabilities";
import { useDemoDraft } from "../../api/demoDraft";

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

type NewRunPageState = {
  readonly form: FormState;
  readonly hydrated: boolean;
  readonly prefillSource: PrefillSource | null;
  readonly draftLoadError: string | null;
  readonly preserveBlankTemplate: boolean;
  readonly templateDefaultingResolved: boolean;
};

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

function pageFormFromDuplicate(
  duplicateState: DuplicateRunLocationState | undefined,
  isDemo: boolean,
  simulatorCapabilities: SimulatorCapabilities | null,
  defaultPageForm: FormState,
): FormState {
  if (!duplicateState) return defaultPageForm;
  if (isDemo && simulatorCapabilities) {
    return formFromDemoDuplicate(
      duplicateState.duplicateRequest,
      simulatorCapabilities,
    );
  }
  return formFromRequest(duplicateState.duplicateRequest);
}

function controllerTypeForProcess(
  current: FormState["controllerType"],
  processType: ProcessType,
  isDemo: boolean,
  simulatorCapabilities: SimulatorCapabilities | null,
): FormState["controllerType"] {
  if (isDemo && simulatorCapabilities) {
    const controllerTypes = demoControllerTypesFor(
      simulatorCapabilities,
      processType,
    );
    if (controllerTypes.includes(current)) return current;
    return demoDefaultControllerTypeFor(simulatorCapabilities, processType);
  }
  if (current === "pid" && !TEMPERATURE_PROCESS_TYPES.has(processType)) {
    return "pi";
  }
  return current;
}

export function NewRunPage({
  capabilities,
}: {
  readonly capabilities: AppCapabilities;
}) {
  const navigate = useNavigate();
  const location = useLocation();
  const isDemo = capabilities.mode === "demo";
  const templates = useTemplates(!isDemo);
  const startRun = useStartRun(isDemo ? "demo" : "full");
  const lastRunRequest = useLastRunRequest(!isDemo);
  const runDraft = useRunDraft(!isDemo);
  const {
    draft: demoDraftValue,
    savedAt: demoDraftSavedAt,
    save: saveDemoDraft,
  } = useDemoDraft(isDemo);
  const saveRunDraft = useSaveRunDraft();
  const duplicateState = isDuplicateRunState(location.state)
    ? location.state
    : undefined;
  const defaultPageForm =
    isDemo && capabilities.simulator
      ? formFromDemoCapabilities(capabilities.simulator)
      : initialForm;
  const initialPageForm = pageFormFromDuplicate(
    duplicateState,
    isDemo,
    capabilities.simulator,
    defaultPageForm,
  );
  const demoDraftForm =
    isDemo && demoDraftValue
      ? capabilities.simulator
        ? formFromDemoDraft(demoDraftValue, capabilities.simulator)
        : defaultPageForm
      : null;
  const resolvedInitialForm =
    duplicateState || !demoDraftForm ? initialPageForm : demoDraftForm;
  const [pageState, setPageState] = useState<NewRunPageState>(() => ({
    form: resolvedInitialForm,
    hydrated: Boolean(duplicateState) || isDemo,
    prefillSource: duplicateState
      ? { kind: "duplicate", runId: duplicateState.duplicateFromRunId }
      : isDemo && demoDraftValue
        ? { kind: "draft" }
        : null,
    draftLoadError: null,
    preserveBlankTemplate: false,
    templateDefaultingResolved:
      Boolean(duplicateState) || Boolean(resolvedInitialForm.template),
  }));
  const {
    form,
    hydrated,
    prefillSource,
    draftLoadError,
    preserveBlankTemplate,
  } = pageState;
  const demoDraftSnapshotRef = useRef(
    JSON.stringify(demoDraftFromForm(resolvedInitialForm)),
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  const [draftSaveError, setDraftSaveError] = useState<string | null>(null);
  const draftSaveChainRef = useRef(Promise.resolve());
  const saveDraftAsync = saveRunDraft.mutateAsync;
  const [tagBrowserOpen, setTagBrowserOpen] = useState(false);
  const activeTemplate = templates.data?.find(
    (template) => template.name === form.template,
  );

  useEffect(() => {
    if (!isDemo || !demoDraftValue) return;
    const nextForm = demoDraftForm ?? defaultPageForm;
    const sanitizedDraft = demoDraftFromForm(nextForm);
    const serializedDraft = JSON.stringify(sanitizedDraft);
    demoDraftSnapshotRef.current = serializedDraft;
    if (JSON.stringify(demoDraftValue) !== serializedDraft) {
      saveDemoDraft(sanitizedDraft, demoDraftSavedAt ?? Date.now());
    }
  }, [
    defaultPageForm,
    demoDraftSavedAt,
    demoDraftForm,
    demoDraftValue,
    isDemo,
    saveDemoDraft,
  ]);

  const fullDraftReady =
    !isDemo &&
    !pageState.hydrated &&
    !duplicateState &&
    !runDraft.isPending &&
    !(runDraft.data === undefined && !runDraft.isError) &&
    !((runDraft.data === null || runDraft.isError) && lastRunRequest.isPending);
  if (fullDraftReady) {
    let nextForm = pageState.form;
    let nextPrefillSource = pageState.prefillSource;
    let nextPreserveBlankTemplate = false;
    if (runDraft.data) {
      nextForm = formFromDraft(runDraft.data);
      nextPrefillSource = { kind: "draft" };
      nextPreserveBlankTemplate = runDraft.data.template === null;
    } else if (lastRunRequest.data) {
      nextForm = formFromRequest(lastRunRequest.data);
      nextPrefillSource = { kind: "last-run" };
    }
    setPageState({
      ...pageState,
      form: nextForm,
      hydrated: true,
      prefillSource: nextPrefillSource,
      draftLoadError: runDraft.isError
        ? userFacingErrorMessage(
            runDraft.error,
            "Unable to load the saved Tune draft; using the available fallback.",
          )
        : null,
      preserveBlankTemplate: nextPreserveBlankTemplate,
      templateDefaultingResolved: nextPreserveBlankTemplate,
    });
  }

  function setForm(action: SetStateAction<FormState>) {
    setPageState((previous) => ({
      ...previous,
      form: typeof action === "function" ? action(previous.form) : action,
    }));
  }

  useEffect(() => {
    if (!hydrated) return;
    const payload = isDemo ? demoDraftFromForm(form) : draftFromForm(form);
    const serializedDemoDraft = isDemo ? JSON.stringify(payload) : undefined;
    if (isDemo && serializedDemoDraft === demoDraftSnapshotRef.current) {
      return;
    }
    const timer = window.setTimeout(() => {
      if (isDemo) {
        if (saveDemoDraft(payload)) {
          demoDraftSnapshotRef.current = serializedDemoDraft!;
        }
        return;
      }
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
  }, [form, hydrated, isDemo, saveDemoDraft, saveDraftAsync]);

  const firstTemplate = templates.data?.[0];
  if (
    !pageState.templateDefaultingResolved &&
    hydrated &&
    !duplicateState &&
    !preserveBlankTemplate &&
    firstTemplate
  ) {
    // Apply the fallback against queued form state so same-render prefill wins.
    setPageState((previous) => {
      if (previous.templateDefaultingResolved) return previous;
      const template = templates.data?.[0];
      const nextForm =
        previous.form.template || !template
          ? previous.form
          : { ...previous.form, template: template.name };
      return {
        ...previous,
        form: nextForm,
        templateDefaultingResolved: true,
      };
    });
  }

  function resetToDefaults() {
    setPageState((previous) => ({
      ...previous,
      form: defaultPageForm,
      prefillSource: null,
      preserveBlankTemplate: false,
      templateDefaultingResolved: false,
    }));
  }

  function resetProcessDefaults() {
    setForm((previous) => ({
      ...previous,
      ...processDefaultsFor(previous.processType),
    }));
  }

  function set<K extends keyof FormState>(key: K, value: FormState[K]) {
    setForm((previous) => ({ ...previous, [key]: value }));
  }

  function setTagName(value: string) {
    setForm((previous) => applyTagNameChange(previous, value));
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
    setPageState((previous) => {
      const template = templates.data?.find((item) => item.name === value);
      const tagname = template
        ? replaceTagSuffix(
            previous.form.tagname,
            template.process_variable_suffix,
          )
        : previous.form.tagname;
      return {
        ...previous,
        form: {
          ...applyTagNameChange(previous.form, tagname),
          template: value,
        },
        templateDefaultingResolved:
          value === "" ? false : previous.templateDefaultingResolved,
      };
    });
  }

  function setProcessType(value: ProcessType) {
    const processDefaults =
      isDemo && capabilities.simulator
        ? demoProcessDefaultsFor(capabilities.simulator, value)
        : processDefaultsFor(value);
    setForm((previous) => ({
      ...previous,
      ...processDefaults,
      processType: value,
      controllerType: controllerTypeForProcess(
        previous.controllerType,
        value,
        isDemo,
        capabilities.simulator,
      ),
    }));
  }

  function submitTune() {
    setValidationError(null);
    const request =
      isDemo && capabilities.simulator
        ? normalizeSimulatorRequest(form, capabilities.simulator)
        : buildRequest(form);
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
        title={isDemo ? "BHTune Simulator Demo" : "New tune"}
        documentationId="new-tune.page"
        description={
          isDemo
            ? "Choose a built-in template and bounded simulator settings, then watch a synthetic MRFT tune."
            : "Configure and start a tune."
        }
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
            {!isDemo && (
              <Button onClick={resetToDefaults}>Reset to defaults</Button>
            )}
          </>
        }
      />

      {!isDemo && prefillSource !== null && (
        <div className="mb-4 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm text-slate-300">
          {prefillMessage(prefillSource)} Change anything below, or "Reset to
          defaults" to return to the built-in defaults.
        </div>
      )}
      {!isDemo && (draftLoadError || draftSaveError) && (
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
              isDemo,
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
        mode={isDemo ? "demo" : "full"}
        simulatorCapabilities={capabilities.simulator ?? undefined}
        form={form}
        template={activeTemplate}
        templates={templates.data}
        templatesPending={templates.isPending}
        onSubmit={handleSubmit}
        onChange={set}
        onTagNameChange={setTagName}
        onDriverChange={setDriver}
        onTemplateChange={setTemplate}
        onProcessTypeChange={setProcessType}
        onResetProcessDefaults={resetProcessDefaults}
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
          onSelect={setTagName}
        />
      )}
    </div>
  );
}
