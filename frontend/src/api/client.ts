import createClient from "openapi-fetch";
import type { paths } from "./schema";

// Relative base URL: in dev, Vite's proxy (see `vite.config.ts`) forwards
// `/api/*` requests to a locally running `bhtune-server`; in production,
// `bhtune-server` serves both the built SPA and the API from the same origin
// (see the `server-embed-spa` phase). Path keys in the generated `schema.ts`
// already include the `/api` prefix (they come straight from the OpenAPI
// spec), so no base path segment is added here — just the origin, which an
// empty string resolves to `fetch`'s current-page default.
export const apiClient = createClient<paths>({
  baseUrl: "",
});
