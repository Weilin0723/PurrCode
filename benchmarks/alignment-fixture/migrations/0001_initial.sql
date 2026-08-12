-- The schema as it stands. Accounts exist; nothing records what they are
-- linked to, which is why `accounts::Accounts` keeps links in memory.
CREATE TABLE IF NOT EXISTS accounts (
    id    TEXT PRIMARY KEY,
    email TEXT NOT NULL
);
