-- Create the restaurants table for the backlog
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS restaurants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id BIGINT NOT NULL,
    name TEXT NOT NULL,
    source_url TEXT,
    google_maps_url TEXT,
    description TEXT,
    cuisine_tags TEXT[] DEFAULT '{}',
    notes TEXT DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    visited BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_restaurants_user_id ON restaurants(user_id);
CREATE INDEX IF NOT EXISTS idx_restaurants_tags ON restaurants USING GIN(cuisine_tags);
CREATE INDEX IF NOT EXISTS idx_restaurants_created_at ON restaurants(user_id, created_at DESC);
