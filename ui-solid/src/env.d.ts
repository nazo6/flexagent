/// <reference types="vite/client" />

/**
 * `unplugin-icons/types/solid` is written for Solid 1 and types every prop as
 * `never` under Solid 2 (`ComponentProps` now lives in `@solidjs/web`).
 * Declare the virtual module against the Solid 2 types instead.
 */
declare module "~icons/*" {
  import type { ComponentProps, JSX } from "@solidjs/web";

  const component: (props: ComponentProps<"svg">) => JSX.Element;
  export default component;
}

/**
 * `monaco-editor` ships no declaration for the basic-languages contribution
 * (only the implementation), so declare the side-effect-only module here.
 */
declare module "monaco-editor/basic-languages/monaco.contribution";
