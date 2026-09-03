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
  demoControllerTypesFor,
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
import type { AppCapabilities } from "../../api/capabilities";
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
  const initialPageForm = duplicateState
    ? isDemo && capabilities.simulator
      ? formFromDemoDuplicate(
          duplicateState.duplicateRequest,
          capabilities.simulator,
        )
      : formFromRequest(duplicateState.duplicateRequest)
    : defaultPageForm;

  const [form, setForm] = useState<FormState>(() => initialPageForm);
  const demoDraftSnapshotRef = useRef(
    JSON.stringify(demoDraftFromForm(initialPageForm)),
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
    if (isDemo) {
      if (hydratedRef.current) return;
      hydratedRef.current = true;
      setHydrated(true);
      if (demoDraftValue) {
        const nextForm = capabilities.simulator
          ? formFromDemoDraft(demoDraftValue, capabilities.simulator)
          : defaultPageForm;
        const sanitizedDraft = demoDraftFromForm(nextForm);
        const serializedDraft = JSON.stringify(sanitizedDraft);
        demoDraftSnapshotRef.current = serializedDraft;
        if (JSON.stringify(demoDraftValue) !== serializedDraft) {
          saveDemoDraft(sanitizedDraft, demoDraftSavedAt ?? Date.now());
        }
        setForm(nextForm);
        setPrefillSource({ kind: "draft" });
      }
      return;
    }
    if (isDemo || hydratedRef.current || duplicateState || runDraft.isPending) {
      return;
    }
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
    capabilities.simulator,
    defaultPageForm,
    duplicateState,
    demoDraftSavedAt,
    demoDraftValue,
    isDemo,
    lastRunRequest.isPending,
    lastRunRequest.data,
    runDraft.data,
    runDraft.error,
    runDraft.isError,
    runDraft.isPending,
    saveDemoDraft,
  ]);

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
    setForm(defaultPageForm);
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
    setForm((previous) => {
      const template = templates.data?.find((item) => item.name === value);
      const tagname = template
        ? replaceTagSuffix(previous.tagname, template.process_variable_suffix)
        : previous.tagname;
      return { ...applyTagNameChange(previous, tagname), template: value };
    });
  }

  function setProcessType(value: ProcessType) {
    setForm((previous) => ({
      ...previous,
      ...(isDemo && capabilities.simulator
        ? demoProcessDefaultsFor(capabilities.simulator, value)
        : processDefaultsFor(value)),
      processType: value,
      controllerType:
        isDemo && capabilities.simulator
          ? demoControllerTypesFor(capabilities.simulator, value).includes(
              previous.controllerType,
            )
            ? previous.controllerType
            : demoControllerTypesFor(capabilities.simulator, value)[0]
          : previous.controllerType === "pid" &&
              !TEMPERATURE_PROCESS_TYPES.has(value)
            ? "pi"
            : previous.controllerType,
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
      {isDemo && (
        <div className="mb-6 rounded-lg border-2 border-amber-700 bg-amber-950/50 px-5 py-4 text-sm text-amber-100">
          <p>
            <strong>Simulator-only demo.</strong> Choose a built-in DCS/PLC
            template, process and controller type, and bounded relay and
            simulator settings.
          </p>
          <p className="mt-2">
            Start a tune to watch the synthetic PV/MV response and review the
            calculated results. The demo never connects to plant equipment or
            writes PID values, and its results must not be used on a live
            control loop.
          </p>
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
