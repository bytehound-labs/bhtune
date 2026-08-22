import { useState } from "react";
import { useOpcServers } from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import { Button, Modal } from "./ui";

/**
 * A "Browse servers on this bridge" affordance for the OPC DA server ProgID field
 * (`ui-opc-browser`): calls `GET /api/opc/servers` on demand and renders the results in a
 * modal so the form stays compact instead of permanently showing every registered server.
 *
 * Deliberately does not fetch automatically on mount -- discovery is a live network call to
 * the bridge gateway, and the New tune form must not make one just because the page loaded.
 * Opening the modal enables the query; closing it removes the list from the form while
 * retaining TanStack Query's short-lived cache for a quick reopen.
 */
export function OpcServerDiscovery({
  bridgeHost,
  onSelect,
}: {
  bridgeHost: string;
  onSelect: (server: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const servers = useOpcServers(bridgeHost, open);

  return (
    <div className="mt-1">
      <Button onClick={() => setOpen(true)} disabled={servers.isFetching}>
        {servers.isFetching ? "Loading servers…" : "Browse servers"}
      </Button>

      {open && (
        <Modal
          title="Browse OPC DA servers"
          onClose={() => setOpen(false)}
          widthClassName="max-w-lg"
        >
          {servers.isPending || servers.isFetching ? (
            <p className="text-sm text-slate-400">Connecting…</p>
          ) : servers.isError ? (
            <p className="text-sm text-red-400">
              {userFacingErrorMessage(
                servers.error,
                "Unable to browse OPC DA servers.",
              )}
            </p>
          ) : !servers.data || servers.data.servers.length === 0 ? (
            <p className="text-sm text-slate-500">No OPC DA servers found.</p>
          ) : (
            <ul className="rounded-md border border-slate-800 bg-slate-950">
              {servers.data.servers.map((server) => (
                <li key={server}>
                  <button
                    type="button"
                    onClick={() => {
                      onSelect(server);
                      setOpen(false);
                    }}
                    className="block w-full truncate px-3 py-2 text-left font-mono text-sm text-slate-300 hover:bg-slate-800"
                  >
                    {server}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Modal>
      )}
    </div>
  );
}
