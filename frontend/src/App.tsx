import { Navigate, Route, Routes } from "react-router";
import { AppLayout } from "./layout/AppLayout";
import { TemplateListPage } from "./routes/templates/TemplateListPage";
import { TemplateDetailPage } from "./routes/templates/TemplateDetailPage";
import { TemplateCreatePage } from "./routes/templates/TemplateCreatePage";
import { TemplateEditPage } from "./routes/templates/TemplateEditPage";
import { RunListPage } from "./routes/history/RunListPage";
import { RunDetailPage } from "./routes/history/RunDetailPage";
import { NewRunPage } from "./routes/runs/NewRunPage";

// Route table for the web GUI (`frontend-screens`). Declarative-mode react-router: no
// loaders, since TanStack Query (wired up in `frontend-shell`) is this project's sole
// data-fetching/caching layer and a second, competing data mechanism would be redundant.
function App() {
  return (
    <Routes>
      <Route element={<AppLayout />}>
        <Route index element={<Navigate to="/templates" replace />} />
        <Route path="templates" element={<TemplateListPage />} />
        <Route path="templates/new" element={<TemplateCreatePage />} />
        <Route path="templates/:name" element={<TemplateDetailPage />} />
        <Route path="templates/:name/edit" element={<TemplateEditPage />} />
        <Route path="runs" element={<RunListPage />} />
        <Route path="runs/new" element={<NewRunPage />} />
        <Route path="runs/:id" element={<RunDetailPage />} />
      </Route>
    </Routes>
  );
}

export default App;
