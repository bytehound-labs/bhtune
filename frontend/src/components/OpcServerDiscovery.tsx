import { useState } from "react";
import { useOpcServers } from "../api/opc";
import { userFacingErrorMessage } from "../api/errors";
import { Button } from "./ui";

/**
 * A "Discover servers on this bridge" affordance for the OPC DA server ProgID field
 * (`ui-opc-browser`): calls `GET /api/opc/servers` on demand and renders the results as a
 * clickable list the engineer can pick from instead of having to already know (or spell
 * correctly) a ProgID like `Matrikon.OPC.Simulation`.
 *
 * Deliberately does not fetch automatically on mount -- discovery is a live network call to
 * the bridge gateway, and the New tune form must not make one just because the page loaded
 * (a fresh install may have no gateway configured, or may not even be using the opcda driver
 * at all). The first click flips on `useOpcServers`'s `enabled` flag; every click after that
 * calls the same query's own `refetch()`, so pointing at a different bridge host and
 * clicking again always re-queries rather than showing a stale list.
 */
export function OpcServerDiscovery({
  bridgeHost,
  onSelect,
}: {
  bridgeHost: string;
  onSelect: (server: string) => void;
}) {
  const [requested, setRequested] = useState(false);
  const servers = useOpcServers(bridgeHost, requested);

  function discover() {
    if (requested) {
      void servers.refetch();
    } else {
      setRequested(true);
    }
  }

  return (
    <div className="mt-1">
      <Button onClick={discover} disabled={servers.isFetching}>
        {servers.isFetching ? "Discovering…" : "Discover servers"}
      </Button>

      {requested && servers.isError && (
        <p className="mt-1 text-xs text-red-400">
          {userFacingErrorMessage(
            servers.error,
            "Unable to discover OPC DA servers.",
          )}
        </p>
      )}

      {requested && servers.data && (
        <>
          {servers.data.servers.length === 0 ? (
            <p className="mt-1 text-xs text-slate-500">
              No OPC DA servers found.
            </p>
          ) : (
            <ul className="mt-1 max-h-32 overflow-y-auto rounded-md border border-slate-800 bg-slate-950">
              {servers.data.servers.map((server) => (
                <li key={server}>
                  <button
                    type="button"
                    onClick={() => onSelect(server)}
                    className="block w-full truncate px-2 py-1 text-left font-mono text-xs text-slate-300 hover:bg-slate-800"
                  >
                    {server}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
