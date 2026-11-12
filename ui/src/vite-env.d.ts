/// <reference types="vite/client" />
/// <reference types="@sveltejs/kit" />

// `monaco-editor` ships no declaration for the basic-languages contribution
// (only the implementation), so declare the side-effect-only module here.
declare module "monaco-editor/basic-languages/monaco.contribution";
