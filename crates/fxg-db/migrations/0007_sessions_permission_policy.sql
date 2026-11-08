-- Per-session approval policy (`ask` / `auto_allow_all`).
ALTER TABLE sessions ADD COLUMN permission_policy TEXT NOT NULL DEFAULT 'ask';
