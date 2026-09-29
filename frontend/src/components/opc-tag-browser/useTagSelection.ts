import { useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { userFacingErrorMessage } from "../../api/errors";
import { useTestOpcConnection } from "../../api/opc";
import type { OpcTagNodeResponse } from "../../api/opc";
import type { components } from "../../api/schema";
import { deriveTag } from "../../lib/opcTags";
import { nodeItemId } from "./browseModel";
import type { QualityWarning, SelectedNode } from "./browseModel";

type TemplateResponse = components["schemas"]["TemplateResponse"];

export function useTagSelection({
  bridgeHost,
  opcServer,
  template,
  onClose,
  onSelect,
  disposeBrowse,
  testConnection,
  selectedNode,
  setSelectedNode,
}: {
  bridgeHost: string;
  opcServer: string;
  template: TemplateResponse | undefined;
  onClose: () => void;
  onSelect: (tag: string) => void;
  disposeBrowse: () => void;
  testConnection: ReturnType<typeof useTestOpcConnection>;
  selectedNode: SelectedNode | null;
  setSelectedNode: Dispatch<SetStateAction<SelectedNode | null>>;
}) {
  const [selectionReadError, setSelectionReadError] = useState<string | null>(
    null,
  );
  const [selectionCheckPending, setSelectionCheckPending] = useState(false);
  const [qualityWarning, setQualityWarning] = useState<QualityWarning | null>(
    null,
  );

  function selectNode(node: OpcTagNodeResponse) {
    const itemId = nodeItemId(node);
    if (!itemId) return;
    setSelectedNode({ nodeKey: node.node_key, itemId });
    setSelectionReadError(null);
    testConnection.reset();
  }

  function applyTag(tag: string) {
    let pvTag = tag;
    if (template) {
      pvTag = deriveTag(tag, template.process_variable_suffix) ?? tag;
    }
    disposeBrowse();
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
    if (!selectedNode) return;
    setSelectionReadError(null);
    testConnection.mutate({
      bridgeHost,
      opcServer,
      tag: selectedNode.itemId,
    });
  }

  function confirmNode(node: OpcTagNodeResponse) {
    const itemId = nodeItemId(node);
    if (itemId) void confirmTag(itemId);
  }

  function closeFromSelection() {
    disposeBrowse();
    onClose();
  }

  function proceedWithQualityWarning() {
    const tag = qualityWarning?.selectedTag;
    if (!tag) return;
    setQualityWarning(null);
    applyTag(tag);
  }

  return {
    qualityWarning,
    setQualityWarning,
    setSelectionReadError,
    selectionCheckPending,
    selectionReadError,
    selectNode,
    confirmTag,
    confirmNode,
    readSelectedTag,
    closeFromSelection,
    proceedWithQualityWarning,
  };
}
