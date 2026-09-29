import { lazy, Suspense, type ReactNode } from "react";
import { Navigate, Route, Routes } from "react-router";
import { AppLayout } from "./layout/AppLayout";
import { useCapabilities } from "./api/capabilities";
import { ErrorBanner, LoadingState } from "./components/ui";
import { userFacingErrorMessage } from "./api/errors";

const TemplateListPage = lazy(() =>
  import("./routes/templates/TemplateListPage").then(
    ({ TemplateListPage }) => ({
      default: TemplateListPage,
    }),
  ),
);
const TemplateDetailPage = lazy(() =>
  import("./routes/templates/TemplateDetailPage").then(
    ({ TemplateDetailPage }) => ({ default: TemplateDetailPage }),
  ),
);
const TemplateCreatePage = lazy(() =>
  import("./routes/templates/TemplateCreatePage").then(
    ({ TemplateCreatePage }) => ({ default: TemplateCreatePage }),
  ),
);
const TemplateEditPage = lazy(() =>
  import("./routes/templates/TemplateEditPage").then(
    ({ TemplateEditPage }) => ({
      default: TemplateEditPage,
    }),
  ),
);
const RunListPage = lazy(() =>
  import("./routes/history/RunListPage").then(({ RunListPage }) => ({
    default: RunListPage,
  })),
);
const RunDetailPage = lazy(() =>
  import("./routes/history/RunDetailPage").then(({ RunDetailPage }) => ({
    default: RunDetailPage,
  })),
);
const NewRunPage = lazy(() =>
  import("./routes/runs/NewRunPage").then(({ NewRunPage }) => ({
    default: NewRunPage,
  })),
);
const ConfigPage = lazy(() =>
  import("./routes/config/ConfigPage").then(({ ConfigPage }) => ({
    default: ConfigPage,
  })),
);

function RouteSuspense({ children }: { readonly children: ReactNode }) {
  // Keep loading feedback in the outlet so the app-wide layout stays mounted.
  return (
    <Suspense fallback={<LoadingState message="Loading page…" />}>
      {children}
    </Suspense>
  );
}

// Route table for the web GUI (`frontend-screens`). Declarative-mode react-router: no
// loaders, since TanStack Query (wired up in `frontend-shell`) is this project's sole
// data-fetching/caching layer and a second, competing data mechanism would be redundant.
function App() {
  const capabilities = useCapabilities();

  if (capabilities.isPending) {
    return <LoadingState message="Loading BHTune capabilities…" />;
  }
  if (capabilities.isError || !capabilities.data) {
    return (
      <ErrorBanner
        message={userFacingErrorMessage(
          capabilities.error,
          "Unable to determine which BHTune features are available.",
        )}
      />
    );
  }
  const appCapabilities = capabilities.data;
  const isDemo = appCapabilities.mode === "demo";

  return (
    <Routes>
      <Route element={<AppLayout capabilities={appCapabilities} />}>
        <Route index element={<Navigate to="/runs/new" replace />} />
        <Route
          path="templates"
          element={
            appCapabilities.actions.manage_templates ? (
              <RouteSuspense>
                <TemplateListPage />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/new"
          element={
            appCapabilities.actions.manage_templates ? (
              <RouteSuspense>
                <TemplateCreatePage />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/:name"
          element={
            appCapabilities.actions.manage_templates ? (
              <RouteSuspense>
                <TemplateDetailPage />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/:name/edit"
          element={
            appCapabilities.actions.manage_templates ? (
              <RouteSuspense>
                <TemplateEditPage />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="runs"
          element={
            appCapabilities.actions.list_history ? (
              <RouteSuspense>
                <RunListPage capabilities={appCapabilities} />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="runs/new"
          element={
            appCapabilities.actions.start_simulator_tune ? (
              <RouteSuspense>
                <NewRunPage capabilities={appCapabilities} />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs" replace />
            )
          }
        />
        <Route
          path="runs/:id"
          element={
            appCapabilities.actions.list_history ? (
              <RouteSuspense>
                <RunDetailPage capabilities={appCapabilities} />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="config"
          element={
            appCapabilities.actions.manage_config ? (
              <RouteSuspense>
                <ConfigPage />
              </RouteSuspense>
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        {isDemo && (
          <Route path="*" element={<Navigate to="/runs/new" replace />} />
        )}
      </Route>
    </Routes>
  );
}

export default App;
