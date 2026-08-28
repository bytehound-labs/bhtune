import { useState } from "react";
import { useOpcServers } from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import { Button, Modal } from "./ui";

interface OpcServerDiscoveryProps {
  readonly bridgeHost: string;
  readonly onSelect: (server: string) => void;
}

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
}: OpcServerDiscoveryProps) {
  const [open, setOpen] = useState(false);
  const servers = useOpcServers(bridgeHost, open);
  const serverContent = (() => {
    if (servers.isPending || servers.isFetching) {
      return <p className="text-sm text-slate-400">Connecting…</p>;
    }

    if (servers.isError) {
      return (
        <p className="text-sm text-red-400">
          {userFacingErrorMessage(
            servers.error,
            "Unable to browse OPC DA servers.",
          )}
        </p>
      );
    }

    if (!servers.data || servers.data.servers.length === 0) {
      return <p className="text-sm text-slate-500">No OPC DA servers found.</p>;
    }

    return (
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
    );
  })();

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
          {serverContent}
        </Modal>
      )}
    </div>
  );
}
