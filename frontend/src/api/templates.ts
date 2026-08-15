import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiClient } from "./client";
import { apiErrorMessage } from "./errors";
import type { components } from "./schema";

export type TemplateResponse = components["schemas"]["TemplateResponse"];
export type DcsTemplate = components["schemas"]["DcsTemplate"];

const templatesKey = ["templates"] as const;
const templateKey = (name: string) => ["templates", name] as const;

/** `GET /api/templates` — every stored template, ordered by name. */
export function useTemplates() {
  return useQuery({
    queryKey: templatesKey,
    queryFn: async () => {
      const { data, error } = await apiClient.GET("/api/templates");
      if (error) throw new Error(apiErrorMessage(error));
      return data;
    },
  });
}

/** `GET /api/templates/{name}` — one template's full detail. */
export function useTemplate(name: string) {
  return useQuery({
    queryKey: templateKey(name),
    queryFn: async () => {
      const { data, error } = await apiClient.GET("/api/templates/{name}", {
        params: { path: { name } },
      });
      if (error) throw new Error(apiErrorMessage(error));
      return data;
    },
    enabled: name.length > 0,
  });
}

/**
 * `POST /api/templates` — always creates a `user`-origin template (see
 * `bhtune-server`'s `routes::templates` doc comment); there is no update endpoint, so
 * editing an existing template means deleting and recreating it.
 */
export function useCreateTemplate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (template: DcsTemplate) => {
      const { data, error } = await apiClient.POST("/api/templates", {
        body: template,
      });
      if (error) {
        throw new Error(apiErrorMessage(error));
      }
      return data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: templatesKey });
    },
  });
}

/** `DELETE /api/templates/{name}` — 409 if still referenced by a saved loop. */
export function useDeleteTemplate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (name: string) => {
      const { error } = await apiClient.DELETE("/api/templates/{name}", {
        params: { path: { name } },
      });
      if (error) {
        throw new Error(apiErrorMessage(error));
      }
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: templatesKey });
    },
  });
}
