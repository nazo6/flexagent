import type { JsonValue } from "$lib/generated/serde_json/JsonValue";

/** 設定項目 (`ConfigOptionInfo`) の選択肢 (Select の value / 表示ラベル / 送信値)。 */
export interface ConfigOptionChoice {
  /** `Select.Item` に渡す安定した文字列表現 (オブジェクトは JSON 文字列) */
  value: string;
  /** 表示ラベル */
  label: string;
  /** `set_config` へ送る元の JSON 値 */
  raw: JsonValue;
}

/** `JsonValue` を Select の value 用文字列へ変換する (文字列以外は JSON 文字列化)。 */
export function configValueToString(value: JsonValue): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** 選択肢の表示ラベルを決める (opencode2 の `{ providerID, id, name }` 等に対応)。 */
function choiceLabel(raw: JsonValue): string {
  if (raw !== null && typeof raw === "object" && !Array.isArray(raw)) {
    const record = raw as Record<string, JsonValue>;
    const name = record.name;
    if (typeof name === "string" && name !== "") return name;
    const provider = record.providerID;
    const id = record.id;
    if (typeof provider === "string" && typeof id === "string") {
      return `${provider}/${id}`;
    }
    if (typeof id === "string" && id !== "") return id;
  }
  return configValueToString(raw);
}

/**
 * `ConfigOptionInfo.options` を Select の選択肢一覧へ変換する。
 *
 * `null` は選択不能として除外する。オブジェクト形式の値 (モデル参照
 * `{ providerID, id, name }` 等) は JSON 文字列化した値を Select の value に
 * 用い、`set_config` 送信時に [`resolveConfigChoice`] で元のオブジェクトへ復元する。
 */
export function configChoices(options: JsonValue): ConfigOptionChoice[] {
  if (!Array.isArray(options)) return [];
  return options.flatMap((raw) =>
    raw === null ? [] : [{ value: configValueToString(raw), label: choiceLabel(raw), raw }],
  );
}

/** Select で選ばれた value を `set_config` へ送る `JsonValue` に解決する。 */
export function resolveConfigChoice(choices: ConfigOptionChoice[], value: string): JsonValue {
  return choices.find((choice) => choice.value === value)?.raw ?? value;
}
