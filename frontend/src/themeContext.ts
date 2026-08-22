import { createContext } from "react";

export type Theme = "light" | "dark";

export type ThemeContextValue = {
  theme: Theme;
  toggleTheme: () => void;
};

export const THEME_STORAGE_KEY = "bhtune-theme";

export const ThemeContext = createContext<ThemeContextValue | undefined>(
  undefined,
);
