<script lang="ts">
  import * as Select from '$lib/components/ui/select';
  import { cn } from '$lib/utils';

  interface Item {
    /** Select の value (送信値) */
    value: string;
    /** トリガ・一覧に表示するラベル */
    label: string;
    /** 一覧で補足表示する説明 (任意) */
    description?: string | null;
  }

  interface Props {
    /** 何を選択する項目なのかを示す固定ラベル (例: モード / モデル) */
    label: string;
    /** 現在値 (選択肢に無い場合は undefined = 未選択表示) */
    value?: string;
    /** 未選択時の表示 */
    placeholder?: string;
    items: Item[];
    disabled?: boolean;
    class?: string;
    onchange: (value: string) => void;
  }

  let {
    label,
    value = undefined,
    placeholder = '未選択',
    items,
    disabled = false,
    class: className = '',
    onchange
  }: Props = $props();
</script>

<!--
  チャット下部の設定セレクタ (モード / モデル等)。
  - トリガには項目名を固定表示し、何を選択するのかを分かるようにする
  - `items` を Root に渡し、トリガと一覧のラベルを常に同じ文字列へ揃える
    (未マウント時の生の値 (mode_id / JSON / "null") 表示を防ぐ)
-->
<Select.Root
  type="single"
  {items}
  {disabled}
  {value}
  onValueChange={(next) => {
    if (typeof next === 'string' && next !== '') onchange(next);
  }}
>
  <Select.Trigger size="sm" class={cn('max-w-64', className)}>
    <span class="text-muted-foreground shrink-0 text-xs">{label}</span>
    <Select.Value {placeholder} />
  </Select.Trigger>
  <Select.Content class="min-w-48">
    {#each items as item (item.value)}
      <Select.Item value={item.value} label={item.label}>
        <span class="flex min-w-0 flex-col">
          <span class="truncate">{item.label}</span>
          {#if item.description}
            <span class="text-muted-foreground max-w-72 truncate text-xs">{item.description}</span>
          {/if}
        </span>
      </Select.Item>
    {/each}
  </Select.Content>
</Select.Root>
