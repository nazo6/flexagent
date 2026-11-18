# Porting contract (`ui/` → `ui-solid/`)

The rules every ported file follows. Written for humans and agents: if the code
here disagrees with this file, fix one of them in the same change.

## Stack

| Piece       | Choice                                                        |
| :---------- | :------------------------------------------------------------ |
| Framework   | `solid-js` 2.0.0 + `@solidjs/web` 2.0.0                       |
| Router      | `@solidjs/router` 2.0.0 (typed `paths`)                       |
| Headless    | `terracotta` 2.0.0 (+ our own geometry)                       |
| Positioning | `@floating-ui/dom` behind `useAnchoredPanel`                  |
| Styles      | Tailwind CSS v4 + `src/styles.css` (Desk tokens)              |
| Icons       | `unplugin-icons` + `@iconify-json/lucide` (`~icons/lucide/*`) |
| i18n        | Paraglide, generated into `src/lib/paraglide`                 |
| Types       | `tsc --noEmit` only (no `svelte-check`)                       |

`solid-js/web` does not exist in Solid 2. Render with `@solidjs/web`
(`render`, `Portal`, `JSX`). JSX types come from `@solidjs/web`; set
`"jsxImportSource": "@solidjs/web"`.

`unplugin-icons/types/solid` types every prop as `never` under Solid 2, so
`src/env.d.ts` declares `~icons/*` against `ComponentProps<"svg">` instead.

## Conversion table

| Svelte 5                   | Solid 2                                                          |
| :------------------------- | :--------------------------------------------------------------- |
| `$state(x)`                | `createSignal(x)`; deep objects → `createStore`                  |
| `$derived(expr)`           | `createMemo(() => expr)`                                         |
| `$effect(fn)`              | `createEffect(compute, apply)`（Solid 2 は分割 API）              |
| `$props()`                 | function argument; `merge` / `omit` for splitting                |
| `$bindable()`              | getter + setter props                                            |
| `{#each list as x (key)}`  | `<For each={list}>` (reference keyed) / `<Index>`                |
| `{#if} {:else}`            | `<Show when fallback>` / `<Switch><Match>`                       |
| `{#snippet}` / `{@render}` | children props / JSX                                             |
| `bind:value`               | `value` + `onInput` (wrapper owns the decision)                  |
| `class:active`             | `classList={{ active }}`                                         |
| `use:action`               | `ref` callback returning a cleanup                               |
| `onMount`                  | `onSettled`; **return** a cleanup, never call `onCleanup` inside |
| class with runes           | factory function returning accessors                             |
| `$app/state` `page.url`    | `useLocation()` / `useSearchParams()`                            |
| `goto(...)`                | `useNavigate()` or a plain `<a href>`                            |
| `+page.ts` `error(404)`    | `matchFilters` on the route                                      |

## Component mapping

| `ui/src/lib/components/ui`                               | `ui-solid/src/lib/components/ui`                    |
| :------------------------------------------------------- | :-------------------------------------------------- |
| dialog, alert-dialog                                     | Terracotta `dialog` / `alert-dialog`                |
| dropdown-menu                                            | `Popover` + `Menu` (submenu: nested `Popover`)      |
| popover                                                  | Terracotta `popover` + `useAnchoredPanel`           |
| select                                                   | `Popover` + `Listbox`                               |
| tabs                                                     | Terracotta `tabs`                                   |
| tooltip                                                  | Terracotta `tooltip` + `useAnchoredPanel`           |
| sonner                                                   | Terracotta `toast` (`ToasterStore` at module scope) |
| drawer, sheet                                            | `Dialog` + CSS (our own)                            |
| scroll-area                                              | native overflow + scrollbar styles (our own)        |
| badge, card, skeleton, separator, input, textarea, label | plain HTML + classes                                |

## Styling state

Terracotta writes `tc-*` attributes instead of `data-state`:

| Old                    | New                                   |
| :--------------------- | :------------------------------------ |
| `data-[state=open]`    | `[&[tc-expanded]]:`                   |
| `data-[selected]`      | `[&[tc-selected]]:`                   |
| `data-[disabled]`      | `[&[tc-disabled]]:`                   |
| `data-[highlighted]`   | `:focus` (menu items move real focus) |
| `data-[state=checked]` | `[&[tc-checked]]:`                    |

Keep `data-slot` on our wrappers: screenshot checks and tests select on it.

## Forbidden

- `any` (and `as unknown as`), `solid-js/web`, `createResource`, `batch`,
  `startTransition`, `produce`, `createMutable`, `createComputed`, `on(` at top
  level of an effect body.
- `onCleanup` inside `onSettled` / `createEffect` (return the cleanup).
- `data-state` based styles outside `ui-solid/src/lib/components/ui`.
- Importing Svelte-only packages (`svelte-sonner`, `mode-watcher`, `vaul-svelte`,
  `@lucide/svelte`, `bits-ui`).
- Reading a URL more than once for an entry-point screen; keep the existing
  "URL is an entry point, not a reactive input" rule with `untrack`.

## Commands

```sh
pnpm --dir ui-solid install
pnpm --dir ui-solid run check   # tsc --noEmit
pnpm --dir ui-solid run build
pnpm --dir ui-solid dev         # http://localhost:5174
```

Gates for every porting PR: `check` clean, `build` clean, no console errors in
the dev server, and a screenshot of the screen at 1100px in light and dark.
