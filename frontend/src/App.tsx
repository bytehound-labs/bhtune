import { Navigate, Route, Routes } from "react-router";
import { AppLayout } from "./layout/AppLayout";
import { TemplateListPage } from "./routes/templates/TemplateListPage";
import { TemplateDetailPage } from "./routes/templates/TemplateDetailPage";
import { TemplateCreatePage } from "./routes/templates/TemplateCreatePage";
import { TemplateEditPage } from "./routes/templates/TemplateEditPage";
import { RunListPage } from "./routes/history/RunListPage";
import { RunDetailPage } from "./routes/history/RunDetailPage";
import { NewRunPage } from "./routes/runs/NewRunPage";
import { ConfigPage } from "./routes/config/ConfigPage";
import { useCapabilities } from "./api/capabilities";
import { ErrorBanner, LoadingState } from "./components/ui";
import { userFacingErrorMessage } from "./api/errors";

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
              <TemplateListPage />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/new"
          element={
            appCapabilities.actions.manage_templates ? (
              <TemplateCreatePage />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/:name"
          element={
            appCapabilities.actions.manage_templates ? (
              <TemplateDetailPage />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="templates/:name/edit"
          element={
            appCapabilities.actions.manage_templates ? (
              <TemplateEditPage />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="runs"
          element={
            appCapabilities.actions.list_history ? (
              <RunListPage capabilities={appCapabilities} />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="runs/new"
          element={
            appCapabilities.actions.start_simulator_tune ? (
              <NewRunPage capabilities={appCapabilities} />
            ) : (
              <Navigate to="/runs" replace />
            )
          }
        />
        <Route
          path="runs/:id"
          element={
            appCapabilities.actions.list_history ? (
              <RunDetailPage capabilities={appCapabilities} />
            ) : (
              <Navigate to="/runs/new" replace />
            )
          }
        />
        <Route
          path="config"
          element={
            appCapabilities.actions.manage_config ? (
              <ConfigPage />
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
