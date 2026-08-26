import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiClient } from "./client";
import { toApiError } from "./errors";
import type { components } from "./schema";

export type DcsTemplate = components["schemas"]["DcsTemplate"];

const templatesKey = ["templates"] as const;
const templateKey = (name: string) => ["templates", name] as const;

/** `GET /api/templates` — every stored template, ordered by name. */
export function useTemplates() {
  return useQuery({
    queryKey: templatesKey,
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET("/api/templates");
      if (error) throw toApiError(error, response);
      return data;
    },
  });
}

/** `GET /api/templates/{name}` — one template's full detail. */
export function useTemplate(name: string) {
  return useQuery({
    queryKey: templateKey(name),
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET(
        "/api/templates/{name}",
        {
          params: { path: { name } },
        },
      );
      if (error) throw toApiError(error, response);
      return data;
    },
    enabled: name.length > 0,
  });
}

/**
 * `POST /api/templates` — always creates a `user`-origin template (see
 * `bhtune-server`'s `routes::templates` doc comment).
 */
export function useCreateTemplate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (template: DcsTemplate) => {
      const { data, error, response } = await apiClient.POST("/api/templates", {
        body: template,
      });
      if (error) {
        throw toApiError(error, response);
      }
      return data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: templatesKey });
    },
  });
}

/**
 * `PUT /api/templates/{name}` — edits an existing `user`-origin template in place; the
 * server rejects renames (400) and edits to `builtin`/`catalog`-origin templates (409),
 * since those are re-seeded from their source file on every startup.
 */
export function useUpdateTemplate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      name,
      template,
    }: {
      name: string;
      template: DcsTemplate;
    }) => {
      const { data, error, response } = await apiClient.PUT(
        "/api/templates/{name}",
        {
          params: { path: { name } },
          body: template,
        },
      );
      if (error) {
        throw toApiError(error, response);
      }
      return data;
    },
    onSuccess: (_data, { name }) => {
      void queryClient.invalidateQueries({ queryKey: templatesKey });
      void queryClient.invalidateQueries({ queryKey: templateKey(name) });
    },
  });
}

/** `DELETE /api/templates/{name}` — 409 if still referenced by a saved loop. */
export function useDeleteTemplate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (name: string) => {
      const { error, response } = await apiClient.DELETE(
        "/api/templates/{name}",
        {
          params: { path: { name } },
        },
      );
      if (error) {
        throw toApiError(error, response);
      }
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: templatesKey });
    },
  });
}
