import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type SetStateAction,
  type SubmitEvent,
} from "react";
import { Link, useLocation, useNavigate } from "react-router";
import type { AppCapabilities } from "../../api/capabilities";
import { useDemoDraft } from "../../api/demoDraft";
import { userFacingErrorMessage } from "../../api/errors";
import {
  useLastRunRequest,
  useRunDraft,
  useRunPreflight,
  useSaveRunDraft,
  useStartRun,
} from "../../api/runs";
import { useTemplates } from "../../api/templates";
import { OpcTagBrowserModal } from "../../components/OpcTagBrowserModal";
import { Button, ErrorBanner, PageHeading } from "../../components/ui";
import { replaceTagSuffix } from "../../lib/opcTags";
import { ConnectionFields } from "./ConnectionFields";
import { DemoRunFields } from "./DemoRunFields";
import { LoopMappingSection } from "./LoopMappingSection";
import {
  SimulatorModelSection,
  SimulatorParameterSection,
} from "./SimulatorFields";
import { TestParameterFields } from "./TestParameterFields";
import { WriteBackFields } from "./WriteBackFields";
import { PreflightReportModal } from "./PreflightReportModal";
import { applyTagNameChange } from "./applyTagNameChange";
import {
  demoDraftFromForm,
  scheduleDemoDraftSave,
  scheduleFullDraftSave,
} from "./draftAutosave";
import {
  buildRequest,
  normalizeSimulatorRequest,
  validationFieldForError,
} from "./formRequest";
import type { ValidationFieldKey } from "./formRequest";
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
import {
  demoProcessDefaultsFor,
  formFromDemoCapabilities,
  initialForm,
  processDefaultsFor,
  templateTagFor,
  templateValueTagFor,
  type FormState,
  type ProcessType,
  type TuneDriver,
} from "./newRunFormState";
import {
  controllerTypeForProcess,
  demoDraftFormFor,
  hydratedFullPageState,
  initialPageState,
  isDuplicateRunState,
  pageFormFromDuplicate,
  prefillMessage,
  resolveTemplateDefaulting,
  shouldResolveTemplateDefaulting,
  type DuplicateRunLocationState,
  type NewRunPageState,
} from "./prefillPrecedence";

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
  const preflight = useRunPreflight();
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
  const defaultPageForm = useMemo(
    () =>
      isDemo && capabilities.simulator
        ? formFromDemoCapabilities(capabilities.simulator)
        : initialForm,
    [capabilities.simulator, isDemo],
  );
  const initialPageForm = pageFormFromDuplicate(
    duplicateState,
    isDemo,
    capabilities.simulator,
    defaultPageForm,
  );
  const demoDraftForm = useMemo(
    () =>
      demoDraftFormFor(
        isDemo,
        demoDraftValue,
        capabilities.simulator,
        defaultPageForm,
      ),
    [capabilities.simulator, defaultPageForm, demoDraftValue, isDemo],
  );
  const resolvedInitialForm =
    duplicateState || !demoDraftForm ? initialPageForm : demoDraftForm;
  const [pageState, setPageState] = useState<NewRunPageState>(() =>
    initialPageState(
      resolvedInitialForm,
      duplicateState,
      isDemo,
      demoDraftValue,
    ),
  );
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
  const validationField = validationError
    ? validationFieldForError(form, validationError)
    : undefined;
  const fieldError = (field: ValidationFieldKey) =>
    validationField === field ? (validationError ?? undefined) : undefined;
  const [draftSaveError, setDraftSaveError] = useState<string | null>(null);
  const draftSaveChainRef = useRef(Promise.resolve());
  const saveDraftAsync = saveRunDraft.mutateAsync;
  const [tagBrowserOpen, setTagBrowserOpen] = useState(false);
  const [acknowledgedIndexErrors, setAcknowledgedIndexErrors] = useState<
    Record<string, readonly string[]>
  >({});
  const indexErrorScope = JSON.stringify([form.bridgeHost, form.server]);
  const rearmIndexErrors = useCallback(() => {
    setAcknowledgedIndexErrors((previous) => ({
      ...previous,
      [indexErrorScope]: [],
    }));
  }, [indexErrorScope]);
  const [preflightOpen, setPreflightOpen] = useState(false);
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

  const hydratedPageState = hydratedFullPageState(
    pageState,
    isDemo,
    duplicateState,
    runDraft,
    lastRunRequest,
  );
  if (hydratedPageState) setPageState(hydratedPageState);

  function setForm(action: SetStateAction<FormState>) {
    preflight.reset();
    setPageState((previous) => ({
      ...previous,
      form: typeof action === "function" ? action(previous.form) : action,
    }));
  }

  const clearDraftSaveError = useCallback(() => {
    setDraftSaveError(null);
  }, []);

  const reportDraftSaveError = useCallback((error: unknown) => {
    setDraftSaveError(
      userFacingErrorMessage(
        error,
        "Unable to save the Tune draft. Changes will remain in this page until the server is available.",
      ),
    );
  }, []);

  useEffect(() => {
    if (!hydrated || isDemo) return;
    return scheduleFullDraftSave(
      form,
      draftSaveChainRef,
      saveDraftAsync,
      clearDraftSaveError,
      reportDraftSaveError,
    );
  }, [
    clearDraftSaveError,
    form,
    hydrated,
    isDemo,
    reportDraftSaveError,
    saveDraftAsync,
  ]);

  useEffect(() => {
    if (!hydrated || !isDemo) return;
    return scheduleDemoDraftSave(form, demoDraftSnapshotRef, saveDemoDraft);
  }, [form, hydrated, isDemo, saveDemoDraft]);

  const firstTemplateName = templates.data?.[0]?.name;
  if (
    shouldResolveTemplateDefaulting(
      pageState,
      hydrated,
      Boolean(duplicateState),
      preserveBlankTemplate,
      firstTemplateName,
    )
  ) {
    // Apply the fallback against queued form state so same-render prefill wins.
    setPageState((previous) =>
      resolveTemplateDefaulting(previous, firstTemplateName),
    );
  }

  function resetToDefaults() {
    preflight.reset();
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
    preflight.reset();
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

  function checkReadiness() {
    preflight.reset();
    setValidationError(null);
    const request = buildRequest(form);
    if (typeof request === "string") {
      setValidationError(request);
      return;
    }
    setPreflightOpen(true);
    preflight.mutate(request);
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
            {!isDemo && (
              <Button
                loading={preflight.isPending}
                disabled={preflight.isPending || startRun.isPending}
                onClick={checkReadiness}
              >
                Check readiness
              </Button>
            )}
            <Button
              variant="primary"
              disabled={startRun.isPending || preflight.isPending}
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

      <form noValidate onSubmit={handleSubmit}>
        {isDemo ? (
          <DemoRunFields
            form={form}
            onChange={set}
            onProcessTypeChange={setProcessType}
            simulatorCapabilities={capabilities.simulator ?? undefined}
            fieldError={fieldError}
          />
        ) : (
          <>
            <ConnectionFields
              form={form}
              templates={templates.data}
              templatesPending={templates.isPending}
              onChange={set}
              onTagNameChange={setTagName}
              onDriverChange={setDriver}
              onTemplateChange={setTemplate}
              onOpenTagBrowser={() => setTagBrowserOpen(true)}
              fieldError={fieldError}
            />
            <TestParameterFields
              form={form}
              onChange={set}
              onProcessTypeChange={setProcessType}
              onResetProcessDefaults={resetProcessDefaults}
              fieldError={fieldError}
            />
            <LoopMappingSection
              form={form}
              template={activeTemplate}
              onTagSourceChange={setTagSource}
              onTagChange={setTagValue}
              onValueSourceChange={setValueSource}
              onValueTagChange={setValueTag}
              onValueChange={setMappingValue}
              onResetTag={resetTag}
              onResetValue={resetValue}
              onResetAll={resetMapping}
              fieldError={fieldError}
            />
            <SimulatorParameterSection
              form={form}
              onChange={set}
              simulatorCapabilities={undefined}
              fieldError={fieldError}
            />
            <WriteBackFields
              form={form}
              onChange={set}
              fieldError={fieldError}
            />
            <SimulatorModelSection
              form={form}
              onChange={set}
              simulatorCapabilities={undefined}
            />
          </>
        )}
      </form>

      {tagBrowserOpen && (
        <OpcTagBrowserModal
          key={indexErrorScope}
          bridgeHost={form.bridgeHost}
          opcServer={form.server}
          template={activeTemplate}
          initialTag={form.tagname}
          acknowledgedIndexErrors={
            acknowledgedIndexErrors[indexErrorScope] ?? []
          }
          onIndexBuildStarted={rearmIndexErrors}
          onClose={(shownIndexErrors) => {
            setAcknowledgedIndexErrors((previous) => ({
              ...previous,
              [indexErrorScope]: [
                ...new Set([
                  ...(previous[indexErrorScope] ?? []),
                  ...shownIndexErrors,
                ]),
              ],
            }));
            setTagBrowserOpen(false);
          }}
          onSelect={setTagName}
        />
      )}
      <PreflightReportModal
        open={preflightOpen}
        pending={preflight.isPending}
        report={preflight.data}
        error={preflight.isError ? preflight.error : null}
        onClose={() => setPreflightOpen(false)}
      />
    </div>
  );
}
