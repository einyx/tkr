//! Per-owner, versioned store of agent manifests. Saving the same name bumps
//! the version; old versions stay so a running session pins its definition.

use anyhow::{anyhow, Result};
use jkr_agent::manifest::Manifest;
use sqlx::PgPool;

pub struct AgentRegistry {
    pool: PgPool,
}

impl AgentRegistry {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Validate + store; returns the new version number.
    pub async fn push(&self, owner: &str, manifest_toml: &str) -> Result<i64> {
        let parsed = Manifest::parse(manifest_toml)
            .map_err(|e| anyhow!("invalid manifest: {e}"))?;
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM agents WHERE owner = $1 AND name = $2",
        )
        .bind(owner)
        .bind(&parsed.name)
        .fetch_one(&self.pool)
        .await?;
        let now = now_secs();
        sqlx::query(
            "INSERT INTO agents (owner, name, version, manifest, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(owner)
        .bind(&parsed.name)
        .bind(next)
        .bind(manifest_toml)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(next)
    }

    /// Load a manifest by name; None version = latest. Owner-scoped.
    pub async fn load(
        &self,
        owner: &str,
        name: &str,
        version: Option<i64>,
    ) -> Result<(i64, Manifest)> {
        let row: Option<(i64, String)> = match version {
            Some(v) => sqlx::query_as(
                "SELECT version, manifest FROM agents \
                 WHERE owner = $1 AND name = $2 AND version = $3",
            )
            .bind(owner)
            .bind(name)
            .bind(v)
            .fetch_optional(&self.pool)
            .await?,
            None => sqlx::query_as(
                "SELECT version, manifest FROM agents \
                 WHERE owner = $1 AND name = $2 ORDER BY version DESC LIMIT 1",
            )
            .bind(owner)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?,
        };
        let (ver, toml) = row.ok_or_else(|| anyhow!("agent not found: {name}"))?;
        let manifest = Manifest::parse(&toml).map_err(|e| anyhow!("invalid manifest: {e}"))?;
        Ok((ver, manifest))
    }

    /// Latest version of each agent owned by `owner`.
    pub async fn list(&self, owner: &str) -> Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT name, MAX(version) FROM agents WHERE owner = $1 GROUP BY name ORDER BY name",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    #[ignore]
    async fn agent_registry_push_load_list() {
        let db_url = match std::env::var("JKR_TEST_DATABASE_URL").ok() {
            Some(u) => u,
            None => {
                eprintln!("skipping: JKR_TEST_DATABASE_URL unset");
                return;
            }
        };

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&db_url)
            .await
            .expect("connect to test DB");

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations");

        let owner = format!("test-{}", uuid::Uuid::new_v4());
        let reg = AgentRegistry::new(pool.clone());
        let toml =
            "name=\"a\"\ntask=\"t\"\n[model]\nprovider=\"anthropic\"\nname=\"m\"\n";

        let v1 = reg.push(&owner, toml).await.unwrap();
        assert_eq!(v1, 1);

        let v2 = reg.push(&owner, toml).await.unwrap();
        assert_eq!(v2, 2);

        let (ver, m) = reg.load(&owner, "a", None).await.unwrap();
        assert_eq!(ver, 2);
        assert_eq!(m.name, "a");

        // specific version
        let (ver1, m1) = reg.load(&owner, "a", Some(1)).await.unwrap();
        assert_eq!(ver1, 1);
        assert_eq!(m1.name, "a");

        // cross-owner isolation
        assert!(reg.load("other-owner-that-does-not-exist", "a", None).await.is_err());

        // list
        let listing = reg.list(&owner).await.unwrap();
        assert_eq!(listing, vec![("a".to_string(), 2i64)]);

        pool.close().await;
    }
}
