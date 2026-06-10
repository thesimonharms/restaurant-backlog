pub mod models;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use self::models::{NewRestaurant, Restaurant};

pub type DbPool = PgPool;

/// Derive a deterministic owner_id UUID from a Telegram/Discord user_id.
/// Uses UUID v5 with a fixed namespace so the same user always gets the same UUID.
/// This is used for RLS scoping and must match the SQL backfill pattern.
pub fn derive_owner_id(user_id: i64) -> Uuid {
    let namespace = Uuid::parse_str("6ba7b811-9dad-11d1-80b4-00c04fd430c8")
        .expect("Invalid namespace UUID");
    let data = format!("restaurant-backlog-user-{user_id}");
    Uuid::new_v5(&namespace, data.as_bytes())
}

/// Initialize the database connection pool
pub async fn init_pool(database_url: &str) -> Result<DbPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(60))
        .connect(database_url)
        .await?;

    // Run migrations (fatal on failure — schema must be correct)
    run_migrations(&pool).await?;

    // Set up RLS policies (non-fatal — runs every startup, idempotent)
    setup_rls(&pool).await;

    Ok(pool)
}

/// Run the SQL migration files
async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    let migration_001 = include_str!("../../migrations/001_create_restaurants.sql");
    sqlx::raw_sql(migration_001).execute(pool).await?;

    let migration_002 = include_str!("../../migrations/002_add_owner_id_rls.sql");
    sqlx::raw_sql(migration_002).execute(pool).await?;

    Ok(())
}

/// Configure Row Level Security policies on the restaurants table.
/// Runs on every startup — uses DROP IF EXISTS so it's fully idempotent.
/// Non-fatal: if Supabase-specific functions aren't available, the bot
/// still works fine (the postgres role bypasses RLS anyway).
async fn setup_rls(pool: &DbPool) {
    let sql = r#"
        ALTER TABLE IF EXISTS restaurants ENABLE ROW LEVEL SECURITY;

        DROP POLICY IF EXISTS "Owner select" ON restaurants;
        DROP POLICY IF EXISTS "Owner insert" ON restaurants;
        DROP POLICY IF EXISTS "Owner update" ON restaurants;
        DROP POLICY IF EXISTS "Owner delete" ON restaurants;

        CREATE POLICY "Owner select" ON restaurants
            FOR SELECT USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
        CREATE POLICY "Owner insert" ON restaurants
            FOR INSERT WITH CHECK (owner_id = current_setting('app.current_owner_id', true)::uuid);
        CREATE POLICY "Owner update" ON restaurants
            FOR UPDATE USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
        CREATE POLICY "Owner delete" ON restaurants
            FOR DELETE USING (owner_id = current_setting('app.current_owner_id', true)::uuid);
    "#;

    match sqlx::raw_sql(sql).execute(pool).await {
        Ok(_) => tracing::info!("RLS policies configured"),
        Err(e) => tracing::warn!("RLS setup skipped (non-fatal): {e}"),
    }
}

/// Save a new restaurant to the backlog
pub async fn save_restaurant(
    pool: &DbPool,
    restaurant: &NewRestaurant,
) -> Result<Restaurant, sqlx::Error> {
    let tags: &[String] = &restaurant.cuisine_tags;

    sqlx::query_as::<_, Restaurant>(
        r#"
        INSERT INTO restaurants (owner_id, user_id, name, source_url, google_maps_url, description, cuisine_tags)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING *
        "#,
    )
    .bind(restaurant.owner_id)
    .bind(restaurant.user_id)
    .bind(&restaurant.name)
    .bind(&restaurant.source_url)
    .bind(&restaurant.google_maps_url)
    .bind(&restaurant.description)
    .bind(tags)
    .fetch_one(pool)
    .await
}

/// Get all restaurants for a user
pub async fn get_user_restaurants(
    pool: &DbPool,
    user_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<Restaurant>, sqlx::Error> {
    sqlx::query_as::<_, Restaurant>(
        r#"
        SELECT * FROM restaurants
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// Get all unique tags for a user
pub async fn get_user_tags(pool: &DbPool, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT unnest(cuisine_tags) as tag
        FROM restaurants
        WHERE user_id = $1
        ORDER BY tag
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Get a random unvisited restaurant for a user
pub async fn get_random_restaurant(
    pool: &DbPool,
    user_id: i64,
) -> Result<Option<Restaurant>, sqlx::Error> {
    sqlx::query_as::<_, Restaurant>(
        r#"
        SELECT * FROM restaurants
        WHERE user_id = $1
        ORDER BY RANDOM()
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

/// Mark a restaurant as visited
pub async fn mark_visited(pool: &DbPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE restaurants SET visited = TRUE WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Search restaurants by tags or name (simple LIKE search)
pub async fn search_restaurants(
    pool: &DbPool,
    user_id: i64,
    query: &str,
) -> Result<Vec<Restaurant>, sqlx::Error> {
    let pattern = format!("%{}%", query);

    sqlx::query_as::<_, Restaurant>(
        r#"
        SELECT * FROM restaurants
        WHERE user_id = $1
        AND (
            name ILIKE $2
            OR description ILIKE $2
            OR $3 = ANY(cuisine_tags)
        )
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(user_id)
    .bind(&pattern)
    .bind(query)
    .fetch_all(pool)
    .await
}

/// Get all restaurants for a user (complete list, for AI recommendations)
pub async fn get_all_restaurants(pool: &DbPool, user_id: i64) -> Result<Vec<Restaurant>, sqlx::Error> {
    sqlx::query_as::<_, Restaurant>(
        r#"
        SELECT * FROM restaurants
        WHERE user_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Delete a restaurant from the backlog
pub async fn delete_restaurant(pool: &DbPool, id: uuid::Uuid, user_id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM restaurants WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete the most recent N restaurants for a user
pub async fn delete_last_restaurants(pool: &DbPool, user_id: i64, n: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        DELETE FROM restaurants
        WHERE id IN (
            SELECT id FROM restaurants
            WHERE user_id = $1
            ORDER BY created_at DESC
            LIMIT $2
        )
        "#,
    )
    .bind(user_id)
    .bind(n)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
