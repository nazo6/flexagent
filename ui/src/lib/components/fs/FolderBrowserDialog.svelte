<script lang="ts">
  import * as Dialog from "#lib/components/ui/dialog/index.ts";
  import { Button } from "#lib/components/ui/button/index.ts";
  import FolderPicker from "./FolderPicker.svelte";
  import type { QuickPath } from "#lib/folder-picker.ts";
  import { m } from "#lib/paraglide/messages.js";
  import FolderOpenIcon from "@lucide/svelte/icons/folder-open";

  interface Props {
    open: boolean;
    nodeId: string;
    initialPath?: string;
    title?: string;
    /** Also show files and allow selecting them (default: folders only) */
    files?: boolean;
    /** Recently used folders shown as quick jumps. */
    quickPaths?: QuickPath[];
    onSelect: (selectedPath: string) => void;
  }

  let {
    open = $bindable(false),
    nodeId,
    initialPath = "",
    title = m.fs_select_folder(),
    files = false,
    quickPaths = [],
    onSelect,
  }: Props = $props();

  let pickerPath = $state("");
  let selectedPath = $state("");
  let pickerLoading = $state(false);

  $effect(() => {
    if (open) selectedPath = "";
  });

  function handleConfirm(): void {
    const target = selectedPath || pickerPath;
    if (target) {
      onSelect(target);
      open = false;
    }
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="flex max-h-[85vh] flex-col gap-3 p-4 sm:max-w-xl">
    <Dialog.Header class="gap-1">
      <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
        <FolderOpenIcon class="text-primary size-4" />
        <span>{title}</span>
      </Dialog.Title>
      <Dialog.Description class="text-muted-foreground text-[13px]">
        {m.fs_description()}
      </Dialog.Description>
    </Dialog.Header>

    <div class="min-h-0 flex-1">
      <FolderPicker
        {nodeId}
        active={open}
        mode="select"
        {initialPath}
        {quickPaths}
        {files}
        class="h-full"
        onpathchange={(path) => (pickerPath = path)}
        onselectionchange={(path) => (selectedPath = path)}
        onloadingchange={(loading) => (pickerLoading = loading)}
      />
    </div>

    <div class="flex items-center gap-2 pt-1 text-[13px]">
      <div class="text-muted-foreground flex min-w-0 items-center gap-1 font-mono text-[11px]">
        <span class="shrink-0">{m.fs_selected()}</span>
        <span
          class="text-foreground truncate font-medium"
          title={selectedPath || pickerPath || m.fs_none_selected()}
        >
          {selectedPath || pickerPath || m.fs_none_selected()}
        </span>
      </div>
    </div>

    <Dialog.Footer class="gap-2 sm:gap-2">
      <Button variant="outline" size="sm" class="h-8 text-[13px]" onclick={() => (open = false)}>
        {m.common_cancel()}
      </Button>
      <Button
        size="sm"
        class="h-8 gap-1.5 text-[13px]"
        disabled={pickerLoading || (!selectedPath && !pickerPath)}
        onclick={handleConfirm}
      >
        <FolderOpenIcon class="size-3.5" />
        <span>{m.fs_select_folder_confirm()}</span>
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
