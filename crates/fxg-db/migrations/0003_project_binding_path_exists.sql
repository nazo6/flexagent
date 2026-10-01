-- プロジェクト × ノード紐付けのローカルパス実在フラグ。
-- ノードが報告 (NodeHello / セッション開始時の登録) した時点のディレクトリ確認結果。
-- 既存行は次回の NodeHello 報告で更新されるまでの暫定として 1 (実在) を設定する。
ALTER TABLE project_node_bindings
    ADD COLUMN path_exists INTEGER NOT NULL DEFAULT 1;
