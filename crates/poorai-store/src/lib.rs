//! SQLite migrations and append-only event persistence.
use poorai_domain::{Id, hash_bytes, now};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage failure: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization failure: {0}")]
    Json(#[from] serde_json::Error),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: Id,
    pub run_id: Option<Id>,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub at: chrono::DateTime<chrono::Utc>,
    pub previous_hash: Option<String>,
    pub event_hash: String,
}
pub struct Store {
    connection: Connection,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }
    fn migrate(&self) -> Result<(), StoreError> {
        self.connection.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY); CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, run_id TEXT, event_type TEXT NOT NULL, payload TEXT NOT NULL, at TEXT NOT NULL, previous_hash TEXT, event_hash TEXT NOT NULL UNIQUE); INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);")?;
        Ok(())
    }
    pub fn append(
        &self,
        run_id: Option<Id>,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<EventRecord, StoreError> {
        let previous_hash = self
            .connection
            .query_row(
                "SELECT event_hash FROM events ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        let at = now();
        let id = poorai_domain::new_id();
        let canonical =
            serde_json::to_vec(&(id, run_id, event_type, &payload, at, &previous_hash))?;
        let event_hash = hash_bytes(canonical);
        self.connection.execute("INSERT INTO events(id,run_id,event_type,payload,at,previous_hash,event_hash) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![id.to_string(), run_id.map(|id| id.to_string()), event_type, serde_json::to_string(&payload)?, at.to_rfc3339(), previous_hash, event_hash])?;
        Ok(EventRecord {
            id,
            run_id,
            event_type: event_type.into(),
            payload,
            at,
            previous_hash,
            event_hash,
        })
    }
    pub fn latest_payload(
        &self,
        run_id: Id,
        event_type: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let mut statement = self.connection.prepare("SELECT payload FROM events WHERE run_id=?1 AND event_type=?2 ORDER BY rowid DESC LIMIT 1")?;
        let mut rows = statement.query(params![run_id.to_string(), event_type])?;
        match rows.next()? {
            Some(row) => {
                let payload: String = row.get(0)?;
                Ok(Some(serde_json::from_str(&payload)?))
            }
            None => Ok(None),
        }
    }
    pub fn events_for_run(&self, run_id: Id) -> Result<Vec<EventRecord>, StoreError> {
        let mut statement=self.connection.prepare("SELECT id,run_id,event_type,payload,at,previous_hash,event_hash FROM events WHERE run_id=?1 ORDER BY rowid")?;
        let rows = statement.query_map(params![run_id.to_string()], |row| {
            let id = uuid::Uuid::parse_str(&row.get::<_, String>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let run = row
                .get::<_, Option<String>>(1)?
                .and_then(|value| uuid::Uuid::parse_str(&value).ok());
            let payload: String = row.get(3)?;
            let at: String = row.get(4)?;
            Ok(EventRecord {
                id,
                run_id: run,
                event_type: row.get(2)?,
                payload: serde_json::from_str(&payload)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                at: chrono::DateTime::parse_from_rfc3339(&at)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?
                    .with_timezone(&chrono::Utc),
                previous_hash: row.get(5)?,
                event_hash: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_chain() {
        let s = Store::open(":memory:").unwrap();
        let a = s.append(None, "a", serde_json::json!({})).unwrap();
        let b = s.append(None, "b", serde_json::json!({})).unwrap();
        assert_eq!(b.previous_hash, Some(a.event_hash));
    }
}
