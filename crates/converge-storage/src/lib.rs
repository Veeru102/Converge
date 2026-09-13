//! Durable storage behind the document actor (`docs/ARCHITECTURE.md` §9).
//!
//! The contract the hub relies on: [`Storage::append`] is idempotent on
//! `(doc, seq)`, and once it returns `Ok` those ops survive a process crash.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use converge_core::Op;
use converge_proto::wire::{decode_op, encode_op};
use converge_proto::DocId;

#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("corrupt op payload at seq {0}: {1}")]
    Corrupt(u64, String),
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// What the actor needs to rebuild a hub.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// Newest snapshot and its seq.
    pub snapshot: Option<(Vec<u8>, u64)>,
    /// Durable ops after the snapshot, contiguous, ascending.
    pub tail: Vec<(u64, Op)>,
}

#[async_trait]
pub trait Storage: Send + Sync + 'static {
    async fn ensure_doc(&self, doc: &DocId) -> Result<()>;
    async fn list_docs(&self) -> Result<Vec<DocId>>;
    async fn load(&self, doc: &DocId) -> Result<Loaded>;
    /// Append accepted ops. Idempotent: an existing `(doc, seq)` is left as is.
    async fn append(&self, doc: &DocId, ops: &[(u64, Op)]) -> Result<()>;
    async fn write_snapshot(&self, doc: &DocId, seq: u64, bytes: &[u8]) -> Result<()>;
    /// Delete ops older than `keep_after` and all but the newest snapshots.
    async fn prune(&self, doc: &DocId, keep_after: u64) -> Result<u64>;
}

// ----- in-memory -----

#[derive(Default)]
struct MemDoc {
    ops: BTreeMap<u64, Op>,
    snapshots: BTreeMap<u64, Vec<u8>>,
}

/// Volatile storage for tests and the simulator-style integration tests.
/// Survives "process restarts" as long as the value itself is kept.
#[derive(Default)]
pub struct MemoryStorage {
    docs: Mutex<BTreeMap<DocId, MemDoc>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    async fn ensure_doc(&self, doc: &DocId) -> Result<()> {
        self.docs.lock().unwrap().entry(doc.clone()).or_default();
        Ok(())
    }
    async fn list_docs(&self) -> Result<Vec<DocId>> {
        Ok(self.docs.lock().unwrap().keys().cloned().collect())
    }
    async fn load(&self, doc: &DocId) -> Result<Loaded> {
        let docs = self.docs.lock().unwrap();
        let Some(d) = docs.get(doc) else {
            return Ok(Loaded::default());
        };
        let snapshot = d.snapshots.iter().next_back().map(|(s, b)| (b.clone(), *s));
        let from = snapshot.as_ref().map(|(_, s)| *s).unwrap_or(0);
        let tail = d
            .ops
            .range(from + 1..)
            .map(|(s, op)| (*s, op.clone()))
            .collect();
        Ok(Loaded { snapshot, tail })
    }
    async fn append(&self, doc: &DocId, ops: &[(u64, Op)]) -> Result<()> {
        let mut docs = self.docs.lock().unwrap();
        let d = docs.entry(doc.clone()).or_default();
        for (seq, op) in ops {
            d.ops.entry(*seq).or_insert_with(|| op.clone());
        }
        Ok(())
    }
    async fn write_snapshot(&self, doc: &DocId, seq: u64, bytes: &[u8]) -> Result<()> {
        self.docs
            .lock()
            .unwrap()
            .entry(doc.clone())
            .or_default()
            .snapshots
            .insert(seq, bytes.to_vec());
        Ok(())
    }
    async fn prune(&self, doc: &DocId, keep_after: u64) -> Result<u64> {
        let mut docs = self.docs.lock().unwrap();
        let Some(d) = docs.get_mut(doc) else {
            return Ok(0);
        };
        let before = d.ops.len();
        d.ops = d.ops.split_off(&keep_after);
        if let Some(newest) = d.snapshots.keys().next_back().copied() {
            d.snapshots.retain(|s, _| *s == newest);
        }
        Ok((before - d.ops.len()) as u64)
    }
}

// ----- Postgres -----

pub struct PgStorage {
    pool: sqlx::PgPool,
}

impl PgStorage {
    /// Connect and run migrations.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(PgStorage { pool })
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
}

#[async_trait]
impl Storage for PgStorage {
    async fn ensure_doc(&self, doc: &DocId) -> Result<()> {
        sqlx::query("INSERT INTO documents (id) VALUES ($1) ON CONFLICT (id) DO NOTHING")
            .bind(&doc.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_docs(&self) -> Result<Vec<DocId>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM documents ORDER BY created_at")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| DocId(id)).collect())
    }

    async fn load(&self, doc: &DocId) -> Result<Loaded> {
        let snap: Option<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, payload FROM snapshots WHERE doc_id = $1 ORDER BY seq DESC LIMIT 1",
        )
        .bind(&doc.0)
        .fetch_optional(&self.pool)
        .await?;
        let from = snap.as_ref().map(|(s, _)| *s).unwrap_or(0);
        let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT seq, payload FROM ops WHERE doc_id = $1 AND seq > $2 ORDER BY seq",
        )
        .bind(&doc.0)
        .bind(from)
        .fetch_all(&self.pool)
        .await?;
        let mut tail = Vec::with_capacity(rows.len());
        for (seq, payload) in rows {
            let op = decode_op(&payload)
                .map_err(|e| StorageError::Corrupt(seq as u64, e.to_string()))?;
            tail.push((seq as u64, op));
        }
        Ok(Loaded {
            snapshot: snap.map(|(s, b)| (b, s as u64)),
            tail,
        })
    }

    async fn append(&self, doc: &DocId, ops: &[(u64, Op)]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO ops (doc_id, seq, replica_id, counter, hlc_wall, hlc_logical, payload) ",
        );
        qb.push_values(ops, |mut b, (seq, op)| {
            b.push_bind(&doc.0)
                .push_bind(*seq as i64)
                .push_bind(op.id.replica.0 as i64)
                .push_bind(op.id.counter as i64)
                .push_bind(op.hlc.wall_ms as i64)
                .push_bind(op.hlc.logical as i32)
                .push_bind(encode_op(op));
        });
        qb.push(" ON CONFLICT (doc_id, seq) DO NOTHING");
        qb.build().execute(&mut *tx).await?;
        let head = ops.last().map(|(s, _)| *s as i64).unwrap_or(0);
        sqlx::query("UPDATE documents SET head_seq = GREATEST(head_seq, $2) WHERE id = $1")
            .bind(&doc.0)
            .bind(head)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn write_snapshot(&self, doc: &DocId, seq: u64, bytes: &[u8]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO snapshots (doc_id, seq, payload) VALUES ($1, $2, $3) ON CONFLICT (doc_id, seq) DO UPDATE SET payload = EXCLUDED.payload")
            .bind(&doc.0).bind(seq as i64).bind(bytes).execute(&mut *tx).await?;
        sqlx::query("UPDATE documents SET snapshot_seq = GREATEST(snapshot_seq, $2) WHERE id = $1")
            .bind(&doc.0)
            .bind(seq as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn prune(&self, doc: &DocId, keep_after: u64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query("DELETE FROM ops WHERE doc_id = $1 AND seq < $2")
            .bind(&doc.0)
            .bind(keep_after as i64)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query("DELETE FROM snapshots WHERE doc_id = $1 AND seq < (SELECT max(seq) FROM snapshots WHERE doc_id = $1)").bind(&doc.0).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use converge_core::*;

    fn op(c: u64) -> Op {
        Op {
            id: OpId::new(9, c),
            hlc: Hlc::new(c, 0),
            kind: OpKind::Create {
                kind: ObjectKind::Rect,
                props: vec![],
            },
        }
    }

    #[tokio::test]
    async fn memory_round_trip() {
        let s = MemoryStorage::new();
        let d = DocId("d".into());
        s.ensure_doc(&d).await.unwrap();
        s.append(&d, &[(1, op(1)), (2, op(2))]).await.unwrap();
        s.append(&d, &[(2, op(99)), (3, op(3))]).await.unwrap(); // seq 2 kept as is
        s.write_snapshot(&d, 1, b"snap").await.unwrap();
        let l = s.load(&d).await.unwrap();
        assert_eq!(l.snapshot, Some((b"snap".to_vec(), 1)));
        assert_eq!(
            l.tail
                .iter()
                .map(|(s, o)| (*s, o.id.counter))
                .collect::<Vec<_>>(),
            vec![(2, 2), (3, 3)]
        );
        assert_eq!(s.prune(&d, 2).await.unwrap(), 1);
    }

    /// Runs only with `DATABASE_URL` (a scratch database).
    #[tokio::test]
    async fn postgres_round_trip() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let s = PgStorage::connect(&url).await.unwrap();
        let d = DocId(format!(
            "test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        s.ensure_doc(&d).await.unwrap();
        s.append(&d, &[(1, op(1)), (2, op(2))]).await.unwrap();
        s.append(&d, &[(2, op(99)), (3, op(3))]).await.unwrap();
        let l = s.load(&d).await.unwrap();
        assert!(l.snapshot.is_none());
        assert_eq!(
            l.tail
                .iter()
                .map(|(s, o)| (*s, o.id.counter))
                .collect::<Vec<_>>(),
            vec![(1, 1), (2, 2), (3, 3)]
        );
        s.write_snapshot(&d, 2, b"snap").await.unwrap();
        let l = s.load(&d).await.unwrap();
        assert_eq!(l.snapshot, Some((b"snap".to_vec(), 2)));
        assert_eq!(l.tail.len(), 1);
        assert_eq!(s.prune(&d, 2).await.unwrap(), 1);
        assert!(s.list_docs().await.unwrap().contains(&d));
    }
}
