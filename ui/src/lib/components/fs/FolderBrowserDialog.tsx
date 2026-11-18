import { createSignal } from "solid-js";
import { Button } from "#lib/components/ui/button/index.ts";
import * as Dialog from "#lib/components/ui/dialog/index.ts";
import type { QuickPath } from "#lib/folder-picker.ts";
import { m } from "#lib/paraglide/messages.js";
import IconFolderOpen from "~icons/lucide/folder-open";
import FolderPicker from "./FolderPicker.tsx";

export interface FolderBrowserDialogProps {
  open: boolean;
  onOpenChange?: (open: boolean) => void;
  nodeId: string;
  initialPath?: string;
  title?: string;
  /** Also show files and allow selecting them (default: folders only) */
  files?: boolean;
  /** Recently used folders shown as quick jumps. */
  quickPaths?: QuickPath[];
  onSelect: (selectedPath: string) => void;
}

/** Modal wrapper around {@link FolderPicker}. */
export default function FolderBrowserDialog(props: FolderBrowserDialogProps) {
  const [pickerPath, setPickerPath] = createSignal("");
  const [selectedPath, setSelectedPath] = createSignal("");
  const [pickerLoading, setPickerLoading] = createSignal(false);

  const close = (): void => {
    // A dialog can be reopened programmatically, which does not emit
    // `onOpenChange`, so clear the pending selection on every close path.
    setSelectedPath("");
    props.onOpenChange?.(false);
  };

  const handleConfirm = (): void => {
    const target = selectedPath() || pickerPath();
    if (target) {
      props.onSelect(target);
      close();
    }
  };

  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={(next) => {
        if (!next) setSelectedPath("");
        props.onOpenChange?.(next);
      }}
    >
      <Dialog.Content class="flex max-h-[85vh] flex-col gap-3 p-4 sm:max-w-xl">
        <Dialog.Header class="gap-1">
          <Dialog.Title class="flex items-center gap-2 text-base font-semibold">
            <IconFolderOpen class="text-primary size-4" />
            <span>{props.title ?? m.fs_select_folder()}</span>
          </Dialog.Title>
          <Dialog.Description class="text-muted-foreground text-[13px]">
            {m.fs_description()}
          </Dialog.Description>
        </Dialog.Header>

        <div class="min-h-0 flex-1">
          <FolderPicker
            nodeId={props.nodeId}
            active={props.open}
            mode="select"
            initialPath={props.initialPath ?? ""}
            quickPaths={props.quickPaths ?? []}
            files={props.files ?? false}
            class="h-full"
            onpathchange={setPickerPath}
            onselectionchange={setSelectedPath}
            onloadingchange={setPickerLoading}
          />
        </div>

        <div class="flex items-center gap-2 pt-1 text-[13px]">
          <div class="text-muted-foreground flex min-w-0 items-center gap-1 font-mono text-[11px]">
            <span class="shrink-0">{m.fs_selected()}</span>
            <span
              class="text-foreground truncate font-medium"
              title={selectedPath() || pickerPath() || m.fs_none_selected()}
            >
              {selectedPath() || pickerPath() || m.fs_none_selected()}
            </span>
          </div>
        </div>

        <Dialog.Footer class="gap-2 sm:gap-2">
          <Button variant="outline" size="sm" class="h-8 text-[13px]" onClick={close}>
            {m.common_cancel()}
          </Button>
          <Button
            size="sm"
            class="h-8 gap-1.5 text-[13px]"
            disabled={pickerLoading() || (!selectedPath() && !pickerPath())}
            onClick={handleConfirm}
          >
            <IconFolderOpen class="size-3.5" />
            <span>{m.fs_select_folder_confirm()}</span>
          </Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
  );
}
