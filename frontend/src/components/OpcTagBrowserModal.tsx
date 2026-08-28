import { useEffect, useRef, useState, type RefObject } from "react";
import { useOpcBrowseFetcher, useTestOpcConnection } from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import type { OpcReadResponse, OpcTagNodeResponse } from "../api/opc";
import type { components } from "../api/schema";
import { SAMPLE_QUALITY_LABELS, SAMPLE_QUALITY_TONE } from "../lib/enumLabels";
import { deriveTag } from "../lib/opcTags";
import { Badge, Button, ErrorBanner, Modal } from "./ui";

type TemplateResponse = components["schemas"]["TemplateResponse"];
type QualityWarning = {
  readonly selectedTag: string;
  readonly reading: OpcReadResponse;
};

type PathState =
  | { status: "loading" }
  | { status: "loaded"; nodes: OpcTagNodeResponse[] }
  | { status: "error"; message: string };

type FetchPath = (path: string) => Promise<OpcTagNodeResponse[]>;
type LoadPath = (path: string) => Promise<OpcTagNodeResponse[] | null>;
type SetPathState = (
  updater: (previous: Record<string, PathState>) => Record<string, PathState>,
) => void;
type SetExpanded = (updater: (previous: Set<string>) => Set<string>) => void;

/** Indentation step per tree depth; matches the width of the expand chevron column so a
 * leaf's label lines up under its parent branch's label, not under its chevron. */
const INDENT_PX = 18;

function namespaceSegments(value: string): string[] {
  return value.split(/[.!/]/).filter((segment) => segment !== "");
}

function isNamespaceDescendant(parent: string, candidate: string): boolean {
  const parentSegments = namespaceSegments(parent);
  const candidateSegments = namespaceSegments(candidate);
  return (
    candidateSegments.length > parentSegments.length &&
    parentSegments.every(
      (segment, index) => candidateSegments[index] === segment,
    )
  );
}

/** One tree level -- renders `pathState[path]`'s nodes and recurses into whichever of them
 * are both a branch and currently expanded. Kept as a separate component (rather than a
 * loop inside `OpcTagBrowserModal` itself) purely so the recursion has somewhere to call
 * back into; all the actual state (`pathState`/`expanded`/`selectedTag`) lives in the parent
 * and is threaded through as props. */
function TreeLevel({
  path,
  depth,
  pathState,
  expanded,
  onToggle,
  onSelect,
  onConfirm,
  selectedTag,
  selectedNodeRef,
  disabled,
}: {
  readonly path: string;
  readonly depth: number;
  readonly pathState: Readonly<Record<string, PathState>>;
  readonly expanded: ReadonlySet<string>;
  readonly onToggle: (tag: string) => void;
  readonly onSelect: (tag: string) => void;
  readonly onConfirm: (tag: string) => void;
  readonly selectedTag: string | null;
  readonly selectedNodeRef: RefObject<HTMLButtonElement | null>;
  readonly disabled: boolean;
}) {
  const state = pathState[path];
  if (!state) return null;

  if (state.status === "loading") {
    return (
      <div
        className="py-1 text-xs text-slate-500"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        Loading…
      </div>
    );
  }
  if (state.status === "error") {
    return (
      <div
        className="py-1 text-xs text-red-400"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        {state.message}
      </div>
    );
  }
  if (state.nodes.length === 0) {
    return (
      <div
        className="py-1 text-xs text-slate-500"
        style={{ paddingLeft: `${depth * INDENT_PX + INDENT_PX}px` }}
      >
        No child tags.
      </div>
    );
  }

  return (
    <>
      {state.nodes.map((node) => (
        <div key={node.tag}>
          <div
            className={`flex items-center gap-1.5 rounded px-1 py-1 text-sm hover:bg-slate-800 ${
              selectedTag === node.tag ? "bg-slate-800" : ""
            }`}
            style={{ paddingLeft: `${depth * INDENT_PX}px` }}
          >
            {node.is_branch ? (
              <button
                type="button"
                onClick={() => onToggle(node.tag)}
                disabled={disabled}
                aria-label={expanded.has(node.tag) ? "Collapse" : "Expand"}
                className="w-4 shrink-0 text-slate-400 hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
              >
                {expanded.has(node.tag) ? "▾" : "▸"}
              </button>
            ) : (
              <span className="w-4 shrink-0" />
            )}
            <button
              type="button"
              onClick={() => onSelect(node.tag)}
              onDoubleClick={() =>
                node.is_branch ? onToggle(node.tag) : onConfirm(node.tag)
              }
              ref={selectedTag === node.tag ? selectedNodeRef : undefined}
              disabled={disabled}
              title={node.tag}
              className="flex-1 truncate text-left font-mono text-slate-200 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {node.tag}
            </button>
            {!node.is_branch && (
              <span className="shrink-0 text-xs text-slate-500">tag</span>
            )}
          </div>
          {node.is_branch && expanded.has(node.tag) && (
            <TreeLevel
              path={node.tag}
              depth={depth + 1}
              pathState={pathState}
              expanded={expanded}
              onToggle={onToggle}
              onSelect={onSelect}
              onConfirm={onConfirm}
              selectedTag={selectedTag}
              selectedNodeRef={selectedNodeRef}
              disabled={disabled}
            />
          )}
        </div>
      ))}
    </>
  );
}

async function loadPath(
  path: string,
  fetchPath: FetchPath,
  setPathState: SetPathState,
): Promise<OpcTagNodeResponse[] | null> {
  setPathState((prev) => ({ ...prev, [path]: { status: "loading" } }));
  try {
    const nodes = await fetchPath(path);
    setPathState((prev) => ({
      ...prev,
      [path]: { status: "loaded", nodes },
    }));
    return nodes;
  } catch (err) {
    setPathState((prev) => ({
      ...prev,
      [path]: {
        status: "error",
        message: userFacingErrorMessage(
          err,
          "Unable to load tags at this level.",
        ),
      },
    }));
    return null;
  }
}

async function revealInitialTag(
  rootNodes: readonly OpcTagNodeResponse[],
  target: string,
  isCancelled: () => boolean,
  load: LoadPath,
  setExpanded: SetExpanded,
  setSelectedTag: (tag: string | null) => void,
): Promise<boolean> {
  let nodes = rootNodes;
  while (!isCancelled()) {
    const matchingNode = nodes.find((node) => node.tag === target);
    if (matchingNode) {
      setSelectedTag(matchingNode.tag);
      return true;
    }

    const branch = nodes.find(
      (node) => node.is_branch && isNamespaceDescendant(node.tag, target),
    );
    if (!branch) return false;

    setExpanded((previous) => {
      if (previous.has(branch.tag)) return previous;
      const next = new Set(previous);
      next.add(branch.tag);
      return next;
    });

    const childNodes = await load(branch.tag);
    if (!childNodes) return false;
    nodes = childNodes;
  }
  return false;
}

async function initializeBrowser({
  initialTag,
  load,
  setExpanded,
  setSelectedTag,
  isCancelled,
}: {
  readonly initialTag: string;
  readonly load: LoadPath;
  readonly setExpanded: SetExpanded;
  readonly setSelectedTag: (tag: string | null) => void;
  readonly isCancelled: () => boolean;
}): Promise<void> {
  const rootNodes = await load("");
  if (isCancelled() || !rootNodes) return;

  const target = initialTag.trim();
  if (
    target &&
    (await revealInitialTag(
      rootNodes,
      target,
      isCancelled,
      load,
      setExpanded,
      setSelectedTag,
    ))
  ) {
    return;
  }
  if (!isCancelled()) {
    setExpanded(() => new Set());
    setSelectedTag(rootNodes[0]?.tag ?? null);
  }
}

function readButtonLabel(isChecking: boolean, isReading: boolean): string {
  if (isChecking) return "Checking…";
  if (isReading) return "Reading…";
  return "Read selected tag";
}

function QualityWarningPanel({
  warning,
  onChooseDifferent,
  onProceed,
}: {
  readonly warning: QualityWarning;
  readonly onChooseDifferent: () => void;
  readonly onProceed: () => void;
}) {
  return (
    <div className="space-y-4">
      <div className="rounded-md border border-amber-800 bg-amber-950/50 p-3 text-sm text-amber-200">
        <p className="font-medium">This tag returned a non-Good OPC quality.</p>
        <p className="mt-2">
          The live value for{" "}
          <span className="font-mono">{warning.selectedTag}</span> was{" "}
          <span className="font-mono">{warning.reading.value}</span> with
          quality{" "}
          <Badge tone={SAMPLE_QUALITY_TONE[warning.reading.quality]}>
            {SAMPLE_QUALITY_LABELS[warning.reading.quality]}
          </Badge>
          .
        </p>
        <p className="mt-2">
          Non-Good values may be stale or invalid. Choose another tag, or
          proceed anyway if you understand the risk.
        </p>
        <p className="mt-2">
          Proceeding only selects this item for the form; a tune still requires
          trustworthy quality for its live readings.
        </p>
      </div>
      <div className="flex justify-end gap-2">
        <Button onClick={onChooseDifferent}>Choose a different tag</Button>
        <Button variant="primary" onClick={onProceed}>
          Proceed anyway
        </Button>
      </div>
    </div>
  );
}

function TagSelectionPanel({
  selectedTag,
  reading,
  readError,
  selectionReadError,
  isReading,
  isChecking,
  onRead,
  onClose,
  onConfirm,
}: {
  readonly selectedTag: string | null;
  readonly reading: OpcReadResponse | undefined;
  readonly readError: unknown;
  readonly selectionReadError: string | null;
  readonly isReading: boolean;
  readonly isChecking: boolean;
  readonly onRead: () => void;
  readonly onClose: () => void;
  readonly onConfirm: () => void;
}) {
  if (!selectedTag) {
    return (
      <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
        <p className="text-sm text-slate-400">
          Select a tag to test its live value and quality.
        </p>
      </div>
    );
  }

  return (
    <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
      <p className="text-sm text-slate-200">
        Selected: <span className="font-mono">{selectedTag}</span>
      </p>
      <p className="mt-1 text-xs text-slate-500">
        Select tag applies the active template&apos;s process-variable suffix.
        Review or override the rest of the mapping in the collapsed section on
        the main tune form.
      </p>

      <div className="mt-3 flex items-center gap-2">
        <Button disabled={isReading || isChecking} onClick={onRead}>
          {readButtonLabel(isChecking, isReading)}
        </Button>
        {reading && (
          <span className="text-xs text-slate-300">
            {reading.value}{" "}
            <Badge tone={SAMPLE_QUALITY_TONE[reading.quality]}>
              {SAMPLE_QUALITY_LABELS[reading.quality]}
            </Badge>
          </span>
        )}
        {readError !== undefined && !selectionReadError && (
          <span className="text-xs text-red-400">
            {userFacingErrorMessage(
              readError,
              "Unable to read the selected tag.",
            )}
          </span>
        )}
        {selectionReadError && <ErrorBanner message={selectionReadError} />}
      </div>

      <div className="mt-3 flex justify-end gap-2">
        <Button onClick={onClose}>Cancel</Button>
        <Button
          variant="primary"
          disabled={isReading || isChecking}
          onClick={onConfirm}
        >
          {isChecking ? "Checking…" : "Select tag"}
        </Button>
      </div>
    </div>
  );
}

function BrowserContent({
  opcServer,
  qualityWarning,
  pathState,
  expanded,
  onToggle,
  onSelect,
  onConfirm,
  selectedTag,
  selectedNodeRef,
  disabled,
  reading,
  readError,
  selectionReadError,
  isReading,
  isChecking,
  onRead,
  onClose,
  onChooseDifferent,
  onProceed,
}: {
  readonly opcServer: string;
  readonly qualityWarning: QualityWarning | null;
  readonly pathState: Readonly<Record<string, PathState>>;
  readonly expanded: ReadonlySet<string>;
  readonly onToggle: (tag: string) => void;
  readonly onSelect: (tag: string) => void;
  readonly onConfirm: (tag: string) => void;
  readonly selectedTag: string | null;
  readonly selectedNodeRef: RefObject<HTMLButtonElement | null>;
  readonly disabled: boolean;
  readonly reading: OpcReadResponse | undefined;
  readonly readError: unknown;
  readonly selectionReadError: string | null;
  readonly isReading: boolean;
  readonly isChecking: boolean;
  readonly onRead: () => void;
  readonly onClose: () => void;
  readonly onChooseDifferent: () => void;
  readonly onProceed: () => void;
}) {
  if (qualityWarning) {
    return (
      <QualityWarningPanel
        warning={qualityWarning}
        onChooseDifferent={onChooseDifferent}
        onProceed={onProceed}
      />
    );
  }
  if (!opcServer) {
    return (
      <p className="text-sm text-slate-400">
        Enter an OPC DA server ProgID above before browsing its tags.
      </p>
    );
  }

  return (
    <>
      <div className="max-h-64 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
        <TreeLevel
          path=""
          depth={0}
          pathState={pathState}
          expanded={expanded}
          onToggle={onToggle}
          onSelect={onSelect}
          onConfirm={onConfirm}
          selectedTag={selectedTag}
          selectedNodeRef={selectedNodeRef}
          disabled={disabled}
        />
      </div>
      <TagSelectionPanel
        selectedTag={selectedTag}
        reading={reading}
        readError={readError}
        selectionReadError={selectionReadError}
        isReading={isReading}
        isChecking={isChecking}
        onRead={onRead}
        onClose={onClose}
        onConfirm={() => onConfirm(selectedTag ?? "")}
      />
    </>
  );
}

/**
 * The OPC tag-tree browser modal (`ui-opc-browser`): a lazily-expanding tree fed one level
 * at a time from `GET /api/opc/browse`, a per-node "Read selected tag" action backed by
 * `GET /api/opc/read`
 * (showing the live value and its quality). When the user confirms a selection, the final
 * component is replaced with the active template's process-variable suffix before the value
 * is written back to the form, but only after a fresh read of the originally selected tag
 * confirms `Good` OPC quality. A non-Good result pauses selection behind an explicit warning,
 * while a read failure leaves the browser open so the tag is never accepted without
 * verification. The main form's collapsed Loop mapping section is the single place for
 * reviewing template defaults and changing any other tag.
 * Double-clicking a leaf performs the same confirmation as the `Select tag` button, while
 * double-clicking a branch expands or collapses it.
 * Reopening the modal starts from `initialTag` when that tag is present in the browsed tree:
 * each matching ancestor branch is expanded and the tag is selected automatically. If the
 * tag is unavailable, the browser keeps its root-level default selection. The selected node
 * is scrolled into the tree viewport when it becomes available.
 *
 * A fresh instance is mounted each time the New tune form opens it (see `NewRunPage`'s
 * conditional render), so there's no need to reset internal state on `bridgeHost`/
 * `opcServer` changes -- those can't change while this is open anyway, since the modal's
 * full-viewport backdrop makes the form underneath unreachable.
 */
export function OpcTagBrowserModal({
  bridgeHost,
  opcServer,
  template,
  initialTag,
  onClose,
  onSelect,
}: {
  readonly bridgeHost: string;
  readonly opcServer: string;
  readonly template: TemplateResponse | undefined;
  readonly initialTag: string;
  readonly onClose: () => void;
  readonly onSelect: (tag: string) => void;
}) {
  const fetchPath = useOpcBrowseFetcher(bridgeHost, opcServer);
  const testConnection = useTestOpcConnection();
  const [pathState, setPathState] = useState<Record<string, PathState>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedTag, setSelectedTag] = useState<string | null>(null);
  const selectedNodeRef = useRef<HTMLButtonElement | null>(null);
  const [selectionReadError, setSelectionReadError] = useState<string | null>(
    null,
  );
  const [selectionCheckPending, setSelectionCheckPending] = useState(false);
  const [qualityWarning, setQualityWarning] = useState<QualityWarning | null>(
    null,
  );

  // Loads the root level once, as soon as there's a server to browse. `opcServer` is the
  // only variable in `fetchPath`'s closure that can plausibly change while this effect's
  // owner is mounted (see the doc comment above for why it otherwise can't) -- an empty
  // `opcServer` means "nothing to browse yet" and is handled by the early-return render
  // path below instead of firing a request that would just 400.
  useEffect(() => {
    if (!opcServer) return;
    let cancelled = false;
    const load = (path: string) => loadPath(path, fetchPath, setPathState);
    void initializeBrowser({
      initialTag,
      load,
      setExpanded,
      setSelectedTag,
      isCancelled: () => cancelled,
    });
    return () => {
      cancelled = true;
    };
    // Deliberately depends only on `bridgeHost`/`opcServer`/`initialTag`: `load`/`fetchPath` are
    // recreated every render but always call through to the same underlying query-cache
    // fetch, so this intentionally does not list them -- doing so would only cause
    // redundant re-fetches of the root level on every unrelated re-render.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [bridgeHost, opcServer, initialTag]);

  useEffect(() => {
    selectedNodeRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedTag, pathState, expanded]);

  function toggle(tag: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(tag)) {
        next.delete(tag);
      } else {
        next.add(tag);
        if (!pathState[tag]) void loadPath(tag, fetchPath, setPathState);
      }
      return next;
    });
  }

  function selectTag(tag: string) {
    setSelectedTag(tag);
    setSelectionReadError(null);
    testConnection.reset();
  }

  function applyTag(tag: string) {
    const pvTag = template
      ? (deriveTag(tag, template.process_variable_suffix) ?? tag)
      : tag;
    onSelect(pvTag);
    onClose();
  }

  async function confirmTag(tag: string) {
    setSelectionReadError(null);
    setSelectionCheckPending(true);
    try {
      const reading = await testConnection.mutateAsync({
        bridgeHost,
        opcServer,
        tag,
      });
      if (reading.quality !== "good") {
        setQualityWarning({ selectedTag: tag, reading });
        return;
      }
      applyTag(tag);
    } catch (err) {
      setSelectionReadError(
        userFacingErrorMessage(
          err,
          "Unable to verify the selected tag's OPC quality.",
        ),
      );
    } finally {
      setSelectionCheckPending(false);
    }
  }

  function readSelectedTag() {
    if (!selectedTag) return;
    setSelectionReadError(null);
    testConnection.mutate({
      bridgeHost,
      opcServer,
      tag: selectedTag,
    });
  }

  return (
    <Modal
      title={
        qualityWarning
          ? "OPC quality warning"
          : `Browse tags on ${opcServer || "(no server)"}`
      }
      onClose={onClose}
      widthClassName="max-w-2xl"
    >
      <BrowserContent
        opcServer={opcServer}
        qualityWarning={qualityWarning}
        pathState={pathState}
        expanded={expanded}
        onToggle={toggle}
        onSelect={selectTag}
        onConfirm={confirmTag}
        selectedTag={selectedTag}
        selectedNodeRef={selectedNodeRef}
        disabled={testConnection.isPending || selectionCheckPending}
        reading={testConnection.isSuccess ? testConnection.data : undefined}
        readError={testConnection.isError ? testConnection.error : undefined}
        selectionReadError={selectionReadError}
        isReading={testConnection.isPending}
        isChecking={selectionCheckPending}
        onRead={readSelectedTag}
        onClose={onClose}
        onChooseDifferent={() => {
          setQualityWarning(null);
        }}
        onProceed={() => {
          if (!qualityWarning) return;
          const tag = qualityWarning.selectedTag;
          setQualityWarning(null);
          applyTag(tag);
        }}
      />
    </Modal>
  );
}
