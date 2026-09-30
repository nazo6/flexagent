import type { BadgeVariant } from "$lib/components/ui/badge";
import type { SessionStatus } from "$lib/generated/SessionStatus";

/** セッション状態の日本語ラベル。 */
export function sessionStatusLabel(status: SessionStatus): string {
  switch (status) {
    case "provisioning":
      return "プロビジョニング";
    case "bootstrapping":
      return "セットアップ中";
    case "idle":
      return "待機";
    case "running":
      return "実行中";
    case "waiting_permission":
      return "承認待ち";
    case "stopped":
      return "停止";
    case "error":
      return "エラー";
  }
}

/** セッション状態の Badge バリアント。 */
export function sessionStatusVariant(status: SessionStatus): BadgeVariant {
  switch (status) {
    case "running":
      return "default";
    case "waiting_permission":
      return "destructive";
    case "error":
      return "destructive";
    case "stopped":
      return "outline";
    default:
      return "secondary";
  }
}

/** 承認リクエストの選択肢種別から日本語ラベルを引く (見つからなければ元の名前)。 */
export function permissionOptionLabel(name: string, kind: string): string {
  if (name) return name;
  switch (kind) {
    case "allow_once":
      return "一度だけ許可";
    case "allow_always":
      return "常に許可";
    case "reject_once":
      return "拒否";
    case "reject_always":
      return "常に拒否";
    default:
      return kind;
  }
}

/** ノード ID を短く表示する (長い自動採番 ID を省略)。 */
export function shortId(id: string, max = 24): string {
  return id.length <= max ? id : `${id.slice(0, max - 1)}…`;
}
