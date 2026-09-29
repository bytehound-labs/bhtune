import type { OpcTagNodeResponse } from "../../api/opc";
import { nodeCanExpand, nodeItemId } from "./browseModel";

export function handleTreeNodeDoubleClick(
  node: OpcTagNodeResponse,
  onToggle: (node: OpcTagNodeResponse) => void,
  onConfirm: (node: OpcTagNodeResponse) => void,
): void {
  if (nodeCanExpand(node)) {
    onToggle(node);
    return;
  }
  if (nodeItemId(node)) {
    onConfirm(node);
    return;
  }
  onToggle(node);
}

export function scrollSelectedNodeIntoView(
  viewport: HTMLDivElement | null,
  selectedNode: HTMLElement | null,
): boolean {
  if (!viewport || !selectedNode) return false;

  const viewportRect = viewport.getBoundingClientRect();
  const selectedRect = selectedNode.getBoundingClientRect();
  const visibleTop = viewportRect.top + viewport.clientTop + 4;
  const visibleBottom =
    viewportRect.top + viewport.clientTop + viewport.clientHeight - 4;

  if (selectedRect.top < visibleTop) {
    viewport.scrollTop -= visibleTop - selectedRect.top;
  } else if (selectedRect.bottom > visibleBottom) {
    viewport.scrollTop += selectedRect.bottom - visibleBottom;
  }

  const settledViewportRect = viewport.getBoundingClientRect();
  const settledSelectedRect = selectedNode.getBoundingClientRect();
  const settledVisibleTop = settledViewportRect.top + viewport.clientTop + 4;
  const settledVisibleBottom =
    settledViewportRect.top + viewport.clientTop + viewport.clientHeight - 4;
  return (
    settledSelectedRect.top >= settledVisibleTop &&
    settledSelectedRect.bottom <= settledVisibleBottom
  );
}
