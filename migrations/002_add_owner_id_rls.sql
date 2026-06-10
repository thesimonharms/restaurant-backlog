-- Add owner_id UUID column for RLS scoping
-- Each Telegram/Discord user gets a deterministic UUID v5 derived from their user_id.
-- This enables Row Level Security so Supabase can enforce per-user access.

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Add the column (nullable initially for backfill)
ALTER TABLE restaurants ADD COLUMN IF NOT EXISTS owner_id UUID;

-- Backfill: assign a deterministic UUID v5 per user_id
-- Same namespace + pattern used in Rust code via uuid::Uuid::new_v5
UPDATE restaurants
SET owner_id = uuid_generate_v5(
    '6ba7b811-9dad-11d1-80b4-00c04fd430c8'::uuid,
    ('restaurant-backlog-user-' || user_id::text)::cstring
)
WHERE owner_id IS NULL;

-- Now make it required
ALTER TABLE restaurants ALTER COLUMN owner_id SET NOT NULL;

-- Index for fast lookups
CREATE INDEX IF NOT EXISTS idx_restaurants_owner_id ON restaurants(owner_id);

-- ── Row Level Security ─────────────────────────────────────────────
-- Since the bot connects as the postgres role (bypasses RLS), these
-- policies don't affect bot operations. They lock down the table for
-- any future Supabase API / anon role access.
ALTER TABLE restaurants ENABLE ROW LEVEL SECURITY;

-- Access is gated by setting app.current_owner_id via SET statement
-- before running queries through the Supabase JS client or REST API.
CREATE POLICY "Owner can select own restaurants" ON restaurants
    FOR SELECT USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
CREATE POLICY "Owner can insert own restaurants" ON restaurants
    FOR INSERT WITH CHECK (owner_id = current_setting('app.current_owner_id', true)::uuid);
CREATE POLICY "Owner can update own restaurants" ON restaurants
    FOR UPDATE USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
CREATE POLICY "Owner can delete own restaurants" ON restaurants
    FOR DELETE USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
