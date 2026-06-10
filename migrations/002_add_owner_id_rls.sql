-- Add owner_id UUID column for RLS scoping
-- Each Telegram/Discord user gets a random UUID assigned on first save.
-- Uses pgcrypto (already installed via migration 001) — no uuid-ossp needed.

-- Add the column (nullable initially for backfill)
ALTER TABLE restaurants ADD COLUMN IF NOT EXISTS owner_id UUID;

-- Backfill existing rows: one random UUID per distinct user_id
-- Uses a PL/pgSQL block to assign a single UUID per user across all their rows
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN SELECT DISTINCT user_id FROM restaurants WHERE owner_id IS NULL LOOP
        UPDATE restaurants SET owner_id = gen_random_uuid()
        WHERE user_id = rec.user_id AND owner_id IS NULL;
    END LOOP;
END;
$$;

-- Now make it required
ALTER TABLE restaurants ALTER COLUMN owner_id SET NOT NULL;

-- Index for fast lookups
CREATE INDEX IF NOT EXISTS idx_restaurants_owner_id ON restaurants(owner_id);
