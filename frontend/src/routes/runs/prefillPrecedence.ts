import type { SimulatorCapabilities } from "../../api/capabilities";
import { userFacingErrorMessage } from "../../api/errors";
import type { NewRunDraft, StartRunRequest } from "../../api/runs";
import { formFromDraft } from "./draftAutosave";
import { formFromRequest } from "./formRequest";
import {
  TEMPERATURE_PROCESS_TYPES,
  demoControllerTypesFor,
  demoDefaultControllerTypeFor,
  formFromDemoDraft,
  formFromDemoDuplicate,
  type FormState,
  type ProcessType,
} from "./newRunFormState";

export type DuplicateRunLocationState = {
  readonly duplicateRequest: StartRunRequest;
  readonly duplicateFromRunId: number;
};

export type PrefillSource =
  | { readonly kind: "duplicate"; readonly runId: number }
  | { readonly kind: "draft" }
  | { readonly kind: "last-run" };

export type NewRunPageState = {
  readonly form: FormState;
  readonly hydrated: boolean;
  readonly prefillSource: PrefillSource | null;
  readonly draftLoadError: string | null;
  readonly preserveBlankTemplate: boolean;
  readonly templateDefaultingResolved: boolean;
};

export function prefillMessage(source: PrefillSource): string {
  switch (source.kind) {
    case "duplicate":
      return `Loaded settings from tune #${source.runId}.`;
    case "draft":
      return "Loaded your saved Tune draft.";
    case "last-run":
      return "Loaded settings from the most recent tune.";
  }
}

export function isDuplicateRunState(
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

export function pageFormFromDuplicate(
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

export function controllerTypeForProcess(
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

export function demoDraftFormFor(
  isDemo: boolean,
  demoDraftValue: NewRunDraft | null,
  simulatorCapabilities: SimulatorCapabilities | null,
  defaultPageForm: FormState,
): FormState | null {
  if (!isDemo || !demoDraftValue) return null;
  if (!simulatorCapabilities) return defaultPageForm;
  return formFromDemoDraft(demoDraftValue, simulatorCapabilities);
}

function initialPrefillSource(
  duplicateState: DuplicateRunLocationState | undefined,
  isDemo: boolean,
  demoDraftValue: NewRunDraft | null,
): PrefillSource | null {
  if (duplicateState) {
    return { kind: "duplicate", runId: duplicateState.duplicateFromRunId };
  }
  if (isDemo && demoDraftValue) return { kind: "draft" };
  return null;
}

export function initialPageState(
  resolvedInitialForm: FormState,
  duplicateState: DuplicateRunLocationState | undefined,
  isDemo: boolean,
  demoDraftValue: NewRunDraft | null,
): NewRunPageState {
  return {
    form: resolvedInitialForm,
    hydrated: Boolean(duplicateState) || isDemo,
    prefillSource: initialPrefillSource(duplicateState, isDemo, demoDraftValue),
    draftLoadError: null,
    preserveBlankTemplate: false,
    templateDefaultingResolved:
      Boolean(duplicateState) || Boolean(resolvedInitialForm.template),
  };
}

export function hydratedFullPageState(
  pageState: NewRunPageState,
  isDemo: boolean,
  duplicateState: DuplicateRunLocationState | undefined,
  runDraft: {
    readonly data: NewRunDraft | null | undefined;
    readonly isPending: boolean;
    readonly isError: boolean;
    readonly error: unknown;
  },
  lastRunRequest: {
    readonly data: StartRunRequest | null | undefined;
    readonly isPending: boolean;
  },
): NewRunPageState | null {
  if (isDemo || pageState.hydrated || duplicateState || runDraft.isPending) {
    return null;
  }
  if (runDraft.data === undefined && !runDraft.isError) return null;
  if (
    (runDraft.data === null || runDraft.isError) &&
    lastRunRequest.isPending
  ) {
    return null;
  }

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
  return {
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
  };
}

export function shouldResolveTemplateDefaulting(
  pageState: NewRunPageState,
  hydrated: boolean,
  hasDuplicate: boolean,
  preserveBlankTemplate: boolean,
  firstTemplateName: string | undefined,
): boolean {
  return (
    !pageState.templateDefaultingResolved &&
    hydrated &&
    !hasDuplicate &&
    !preserveBlankTemplate &&
    Boolean(firstTemplateName)
  );
}

export function resolveTemplateDefaulting(
  previous: NewRunPageState,
  firstTemplateName: string | undefined,
): NewRunPageState {
  if (previous.templateDefaultingResolved) return previous;
  if (previous.form.template || !firstTemplateName) {
    return { ...previous, templateDefaultingResolved: true };
  }
  return {
    ...previous,
    form: { ...previous.form, template: firstTemplateName },
    templateDefaultingResolved: true,
  };
}
