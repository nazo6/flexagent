import { encodeBytesToBase64 } from "#lib/base64.ts";
import type { AttachmentMeta } from "#lib/generated/bindings.ts";
import { m } from "#lib/paraglide/messages.js";
import { toast } from "svelte-sonner";

/** Inline attachment upload limit (matches the node-side limit). */
const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

/** Best-effort MIME type from a file name (mirrors the CLI helper). */
function guessMime(fileName: string): string {
  const extension = fileName.split(".").pop()?.toLowerCase() ?? "";
  switch (extension) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "svg":
      return "image/svg+xml";
    case "pdf":
      return "application/pdf";
    case "json":
      return "application/json";
    case "md":
      return "text/markdown";
    case "txt":
    case "log":
      return "text/plain";
    default:
      return "application/octet-stream";
  }
}

/**
 * Attachment state shared by the new-session and session composers.
 *
 * Images are transported inline (`data_b64`); local files are referenced by
 * their absolute path on the node.
 */
export function createComposerAttachments() {
  let attachments = $state<AttachmentMeta[]>([]);

  async function addImageFiles(files: FileList | File[]): Promise<void> {
    for (const file of Array.from(files)) {
      if (!file.type.startsWith("image/")) {
        toast.error(m.chat_attachment_image_only());
        continue;
      }
      if (file.size > MAX_ATTACHMENT_BYTES) {
        toast.error(m.chat_attachment_too_large({ limit: MAX_ATTACHMENT_BYTES / (1024 * 1024) }));
        continue;
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      attachments.push({
        file_name: file.name || "image",
        mime_type: file.type || guessMime(file.name),
        size_bytes: file.size,
        local_path: null,
        data_b64: encodeBytesToBase64(bytes),
      });
    }
  }

  function addLocalFile(path: string): void {
    const fileName = path.split(/[\\/]/).pop() ?? "attachment";
    attachments.push({
      file_name: fileName,
      mime_type: guessMime(fileName),
      size_bytes: 0,
      local_path: path,
      data_b64: null,
    });
  }

  function remove(index: number): void {
    attachments = attachments.filter((_, current) => current !== index);
  }

  function clear(): void {
    attachments = [];
  }

  function onPaste(event: ClipboardEvent): void {
    const files = event.clipboardData?.files;
    if (files && files.length > 0) {
      event.preventDefault();
      void addImageFiles(files);
    }
  }

  function onDrop(event: DragEvent): void {
    event.preventDefault();
    const files = event.dataTransfer?.files;
    if (files && files.length > 0) {
      void addImageFiles(files);
    }
  }

  return {
    get attachments(): AttachmentMeta[] {
      return attachments;
    },
    addImageFiles,
    addLocalFile,
    remove,
    clear,
    onPaste,
    onDrop,
  };
}
