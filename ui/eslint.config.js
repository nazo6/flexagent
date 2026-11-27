import js from "@eslint/js";
import paraglide from "@inlang/paraglide-js/eslint";
import vitest from "@vitest/eslint-plugin";
import svelte from "eslint-plugin-svelte";
import globals from "globals";
import tseslint from "typescript-eslint";

export default [
  {
    ignores: [
      "build/**",
      ".svelte-kit/**",
      "node_modules/**",
      "src/lib/generated/**",
      "src/lib/paraglide/**",
    ],
  },

  // Base rules; Oxlint's categories used to supply these implicitly.
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...svelte.configs["flat/recommended"],

  // Svelte files need TypeScript inside `<script lang="ts">` and runes modules.
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: { parserOptions: { parser: tseslint.parser } },
  },

  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: {
      // Plain collections are used where reactivity is deliberately unnecessary
      // (lookup caches, gesture state); SvelteMap/Set stays a per-case decision.
      "svelte/prefer-svelte-reactivity": "off",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
    },
  },

  {
    files: ["**/*.svelte"],
    rules: {
      // Markdown and highlighted code are sanitized before `{@html}`.
      "svelte/no-at-html-tags": "off",
      // Scroll, focus and terminal integration need direct DOM access.
      "svelte/no-dom-manipulating": "off",
      // Migrating every link/goto to SvelteKit's resolve() is tracked separately.
      "svelte/no-navigation-without-resolve": "off",
      // Fold compiler warnings into lint instead of leaving them to svelte-check.
      "svelte/valid-compile": "error",
    },
  },

  {
    files: ["src/**/*.{ts,svelte}"],
    plugins: { paraglide },
    rules: {
      "paraglide/no-dynamic-message-access": ["error", { messageModuleDirectories: ["paraglide"] }],
    },
  },

  {
    files: ["src/**/*.test.ts", "src/**/*.test.svelte.ts"],
    ...vitest.configs.recommended,
    languageOptions: { globals: vitest.environments.env.globals },
  },
];
