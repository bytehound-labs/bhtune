import { useEffect, useState } from "react";
import { useOpcBrowseFetcher, useTestOpcConnection } from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import type { OpcReadResponse, OpcTagNodeResponse } from "../api/opc";
import type { components } from "../api/schema";
import { SAMPLE_QUALITY_LABELS, SAMPLE_QUALITY_TONE } from "../lib/enumLabels";
import { deriveTag, derivedTagPreview } from "../lib/opcTags";
import { Badge, Button, ErrorBanner, Modal } from "./ui";

type TemplateResponse = components["schemas"]["TemplateResponse"];
type QualityWarning = {
  selectedTag: string;
  reading: OpcReadResponse;
};

type PathState =
  | { status: "loading" }
  | { status: "loaded"; nodes: OpcTagNodeResponse[] }
  | { status: "error"; message: string };

/** Indentation step per tree depth; matches the width of the expand chevron column so a
 * leaf's label lines up under its parent branch's label, not under its chevron. */
const INDENT_PX = 18;

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
  disabled,
}: {
  path: string;
  depth: number;
  pathState: Record<string, PathState>;
  expanded: Set<string>;
  onToggle: (tag: string) => void;
  onSelect: (tag: string) => void;
  onConfirm: (tag: string) => void;
  selectedTag: string | null;
  disabled: boolean;
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
              disabled={disabled}
            />
          )}
        </div>
      ))}
    </>
  );
}

/**
 * The OPC tag-tree browser modal (`ui-opc-browser`): a lazily-expanding tree fed one level
 * at a time from `GET /api/opc/browse`, a per-node "Read selected tag" action backed by
 * `GET /api/opc/read`
 * (showing the live value and its quality), and a preview of the active template's full
 * derived tag set for whichever node is selected -- the clearest available explanation of
 * how a template's suffixes actually work, since it shows the real tag names that would
 * result from the exact tag just picked (see `derivedTagPreview`'s doc comment for why
 * *any* node under a loop's hierarchy, not just its PV leaf, yields the identical set).
 * When the user confirms a selection, the final component is replaced with the active
 * template's process-variable suffix before the value is written back to the form, but only
 * after a fresh read of the originally selected tag confirms `Good` OPC quality. A non-Good
 * result pauses selection behind an explicit warning, while a read failure leaves the browser
 * open so the tag is never accepted without verification.
 * Double-clicking a leaf performs the same confirmation as the `Select tag` button, while
 * double-clicking a branch expands or collapses it.
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
  onClose,
  onSelect,
}: {
  bridgeHost: string;
  opcServer: string;
  template: TemplateResponse | undefined;
  onClose: () => void;
  onSelect: (tag: string) => void;
}) {
  const fetchPath = useOpcBrowseFetcher(bridgeHost, opcServer);
  const testConnection = useTestOpcConnection();
  const [pathState, setPathState] = useState<Record<string, PathState>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedTag, setSelectedTag] = useState<string | null>(null);
  const [selectionReadError, setSelectionReadError] = useState<string | null>(
    null,
  );
  const [selectionCheckPending, setSelectionCheckPending] = useState(false);
  const [qualityWarning, setQualityWarning] = useState<QualityWarning | null>(
    null,
  );

  async function load(path: string) {
    setPathState((prev) => ({ ...prev, [path]: { status: "loading" } }));
    try {
      const nodes = await fetchPath(path);
      setPathState((prev) => ({
        ...prev,
        [path]: { status: "loaded", nodes },
      }));
      if (path === "" && nodes.length > 0) {
        setSelectedTag((previous) => previous ?? nodes[0].tag);
      }
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
    }
  }

  // Loads the root level once, as soon as there's a server to browse. `opcServer` is the
  // only variable in `fetchPath`'s closure that can plausibly change while this effect's
  // owner is mounted (see the doc comment above for why it otherwise can't) -- an empty
  // `opcServer` means "nothing to browse yet" and is handled by the early-return render
  // path below instead of firing a request that would just 400.
  useEffect(() => {
    if (!opcServer) return;
    void load("");
    // Deliberately depends only on `bridgeHost`/`opcServer`: `load`/`fetchPath` are
    // recreated every render but always call through to the same underlying query-cache
    // fetch, so this intentionally does not list them -- doing so would only cause
    // redundant re-fetches of the root level on every unrelated re-render.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [bridgeHost, opcServer]);

  function toggle(tag: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(tag)) {
        next.delete(tag);
      } else {
        next.add(tag);
        if (!pathState[tag]) void load(tag);
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

  const preview =
    selectedTag && template ? derivedTagPreview(selectedTag, template) : null;
  const selectedPvTag =
    selectedTag && template
      ? (deriveTag(selectedTag, template.process_variable_suffix) ??
        selectedTag)
      : selectedTag;

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
      {qualityWarning ? (
        <div className="space-y-4">
          <div className="rounded-md border border-amber-800 bg-amber-950/50 p-3 text-sm text-amber-200">
            <p className="font-medium">
              This tag returned a non-Good OPC quality.
            </p>
            <p className="mt-2">
              The live value for{" "}
              <span className="font-mono">{qualityWarning.selectedTag}</span>{" "}
              was{" "}
              <span className="font-mono">{qualityWarning.reading.value}</span>{" "}
              with quality{" "}
              <Badge tone={SAMPLE_QUALITY_TONE[qualityWarning.reading.quality]}>
                {SAMPLE_QUALITY_LABELS[qualityWarning.reading.quality]}
              </Badge>
              .
            </p>
            <p className="mt-2">
              Non-Good values may be stale or invalid. Choose another tag, or
              proceed anyway if you understand the risk.
            </p>
            <p className="mt-2">
              Proceeding only selects this item for the form; a tune still
              requires trustworthy quality for its live readings.
            </p>
          </div>
          <div className="flex justify-end gap-2">
            <Button onClick={() => setQualityWarning(null)}>
              Choose a different tag
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                const tag = qualityWarning.selectedTag;
                setQualityWarning(null);
                applyTag(tag);
              }}
            >
              Proceed anyway
            </Button>
          </div>
        </div>
      ) : !opcServer ? (
        <p className="text-sm text-slate-400">
          Enter an OPC DA server ProgID above before browsing its tags.
        </p>
      ) : (
        <>
          <div className="max-h-64 overflow-y-auto rounded-md border border-slate-800 bg-slate-950 p-2">
            <TreeLevel
              path=""
              depth={0}
              pathState={pathState}
              expanded={expanded}
              onToggle={toggle}
              onSelect={selectTag}
              onConfirm={confirmTag}
              selectedTag={selectedTag}
              disabled={testConnection.isPending || selectionCheckPending}
            />
          </div>

          <div className="mt-4 min-h-[10rem] rounded-md border border-slate-700 bg-slate-900 p-3">
            {!selectedTag ? (
              <p className="text-sm text-slate-400">
                Select a tag to preview its template mapping.
              </p>
            ) : (
              <>
                <p className="text-sm text-slate-200">
                  Selected: <span className="font-mono">{selectedTag}</span>
                </p>
                {selectedPvTag && selectedPvTag !== selectedTag && (
                  <p className="mt-1 text-xs text-slate-400">
                    PV tag: <span className="font-mono">{selectedPvTag}</span>
                  </p>
                )}

                {template ? (
                  <details className="mt-2">
                    <summary className="cursor-pointer select-none text-xs uppercase tracking-wide text-slate-500">
                      Detected tags (template: {template.name})
                    </summary>
                    <ul className="mt-2 space-y-0.5 font-mono text-xs text-slate-400">
                      {preview?.map((row) => (
                        <li key={row.label}>
                          <span className="text-slate-500">{row.label}:</span>{" "}
                          {row.tag ?? (
                            <span className="italic text-slate-600">
                              not used by this template
                            </span>
                          )}
                        </li>
                      ))}
                    </ul>
                  </details>
                ) : (
                  <p className="mt-1 text-xs text-slate-500">
                    Choose a template above to preview its full derived tag set.
                  </p>
                )}

                <div className="mt-3 flex items-center gap-2">
                  <Button
                    disabled={testConnection.isPending || selectionCheckPending}
                    onClick={readSelectedTag}
                  >
                    {selectionCheckPending
                      ? "Checking…"
                      : testConnection.isPending
                        ? "Reading…"
                        : "Read selected tag"}
                  </Button>
                  {testConnection.isSuccess && (
                    <span className="text-xs text-slate-300">
                      {testConnection.data.value}{" "}
                      <Badge
                        tone={SAMPLE_QUALITY_TONE[testConnection.data.quality]}
                      >
                        {SAMPLE_QUALITY_LABELS[testConnection.data.quality]}
                      </Badge>
                    </span>
                  )}
                  {testConnection.isError && !selectionReadError && (
                    <span className="text-xs text-red-400">
                      {userFacingErrorMessage(
                        testConnection.error,
                        "Unable to read the selected tag.",
                      )}
                    </span>
                  )}
                  {selectionReadError && (
                    <ErrorBanner message={selectionReadError} />
                  )}
                </div>

                <div className="mt-3 flex justify-end gap-2">
                  <Button onClick={onClose}>Cancel</Button>
                  <Button
                    variant="primary"
                    disabled={testConnection.isPending || selectionCheckPending}
                    onClick={() => void confirmTag(selectedTag)}
                  >
                    {selectionCheckPending ? "Checking…" : "Select tag"}
                  </Button>
                </div>
              </>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}
