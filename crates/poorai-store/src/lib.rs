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
/// Whether a run's chain holds, and where it does not.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainVerdict {
    pub events: usize,
    /// The one-based position of the first event whose hash or link does not
    /// hold. `None` means every link checked out.
    pub broken_at: Option<usize>,
    /// Events written before the run chain existed, which carry no link to
    /// verify. Counted rather than passed over, so "verified" never quietly
    /// means "there was nothing to check".
    pub unlinked: usize,
}

impl ChainVerdict {
    /// Whether the chain holds *and* there was something to check.
    pub fn intact(&self) -> bool {
        self.broken_at.is_none() && self.events > 0 && self.unlinked < self.events
    }
}

/// One named session, reconstructed from the events that opened it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub name: String,
    /// Workspace the session was opened against, as recorded at the time.
    pub root: String,
    /// Every run of this session, oldest first.
    pub runs: Vec<Id>,
    pub last_opened_at: chrono::DateTime<chrono::Utc>,
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
        // Migration 2: a chain per run, beside the global one.
        //
        // The global chain links every event to whatever was appended last,
        // whichever run that belonged to. So a run's events depend on runs
        // interleaved with them, two runs in one database cannot be verified
        // independently, and a run's trail cannot be carried anywhere without
        // carrying every run beside it. Both chains are kept: the global one
        // still orders the database, and the per-run one is what makes a
        // single run's evidence stand on its own.
        //
        // `ALTER TABLE` is guarded rather than versioned-out, because a
        // database written before this column exists is the normal case and
        // its rows are honestly unverifiable on the run chain rather than
        // corrupt. The verifier says which.
        let has_column: bool = self
            .connection
            .prepare("SELECT 1 FROM pragma_table_info('events') WHERE name='run_previous_hash'")?
            .exists([])?;
        if !has_column {
            self.connection
                .execute_batch("ALTER TABLE events ADD COLUMN run_previous_hash TEXT;")?;
        }
        self.connection
            .execute_batch("INSERT OR IGNORE INTO schema_migrations(version) VALUES (2);")?;
        Ok(())
    }

    /// Walks one run's chain and says whether it holds.
    ///
    /// The API only ever appended, and SQLite permits `UPDATE` and `DELETE`
    /// regardless -- so "append-only" was a property of the code rather than
    /// of the data, and nothing could tell the difference after the fact.
    /// Recomputing each event's hash from what is stored is what turns the
    /// chain from a decoration into evidence.
    pub fn verify_run_chain(&self, run_id: Id) -> Result<ChainVerdict, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id,event_type,payload,at,previous_hash,run_previous_hash,event_hash FROM events WHERE run_id=?1 ORDER BY rowid",
        )?;
        let rows = statement.query_map(params![run_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let mut verdict = ChainVerdict::default();
        let mut expected_previous: Option<String> = None;
        for row in rows {
            let (id, event_type, payload, at, previous_hash, run_previous_hash, event_hash) = row?;
            verdict.events += 1;
            let Some(id) = uuid::Uuid::parse_str(&id).ok() else {
                verdict.broken_at.get_or_insert(verdict.events);
                continue;
            };
            let payload: serde_json::Value = serde_json::from_str(&payload)?;
            let Ok(at) = chrono::DateTime::parse_from_rfc3339(&at) else {
                verdict.broken_at.get_or_insert(verdict.events);
                continue;
            };
            let at = at.with_timezone(&chrono::Utc);
            let canonical = serde_json::to_vec(&(
                id,
                Some(run_id),
                event_type.as_str(),
                &payload,
                at,
                &previous_hash,
            ))?;
            if hash_bytes(canonical) != event_hash {
                verdict.broken_at.get_or_insert(verdict.events);
                continue;
            }
            match &run_previous_hash {
                // Written before the run chain existed. Not a break: an
                // absence of evidence said plainly rather than reported as
                // tampering.
                None => verdict.unlinked += 1,
                Some(_) if run_previous_hash != expected_previous => {
                    verdict.broken_at.get_or_insert(verdict.events);
                }
                Some(_) => {}
            }
            expected_previous = Some(event_hash);
        }
        Ok(verdict)
    }
    /// Appends a typed event.
    ///
    /// The preferred entry point: the event type and its payload are derived
    /// from one value rather than written as a literal and a hand-built object
    /// at each call site, so two places recording the same event cannot
    /// disagree about its shape.
    pub fn append_event(
        &self,
        run_id: Option<Id>,
        event: &poorai_domain::RunEvent,
    ) -> Result<EventRecord, StoreError> {
        self.append(run_id, event.event_type(), event.payload())
    }

    /// Reads a run's events back as typed values.
    ///
    /// An event this build does not know is skipped: it was written by another
    /// version, and a reducer that guesses at it resumes into a state nothing
    /// recorded.
    pub fn typed_events_for_run(
        &self,
        run_id: Id,
    ) -> Result<Vec<poorai_domain::RunEvent>, StoreError> {
        Ok(self
            .events_for_run(run_id)?
            .iter()
            .filter_map(|record| {
                poorai_domain::RunEvent::from_stored(&record.event_type, &record.payload)
            })
            .collect())
    }

    pub fn append(
        &self,
        run_id: Option<Id>,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<EventRecord, StoreError> {
        let previous_hash: Option<String> = self
            .connection
            .query_row(
                "SELECT event_hash FROM events ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        // The previous event *of this run*, so a run's evidence stands on its
        // own rather than depending on whatever else the database held.
        let run_previous_hash: Option<String> = run_id.and_then(|run_id| {
            self.connection
                .query_row(
                    "SELECT event_hash FROM events WHERE run_id=?1 ORDER BY rowid DESC LIMIT 1",
                    params![run_id.to_string()],
                    |row| row.get(0),
                )
                .ok()
        });
        let at = now();
        let id = poorai_domain::new_id();
        let canonical =
            serde_json::to_vec(&(id, run_id, event_type, &payload, at, &previous_hash))?;
        let event_hash = hash_bytes(canonical);
        self.connection.execute("INSERT INTO events(id,run_id,event_type,payload,at,previous_hash,run_previous_hash,event_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id.to_string(), run_id.map(|id| id.to_string()), event_type, serde_json::to_string(&payload)?, at.to_rfc3339(), previous_hash, run_previous_hash, event_hash])?;
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
    /// A session as it can be reconstructed: its name, the runs that carried
    /// it in order, and when it was last touched.
    ///
    /// Sessions are derived from the event log rather than kept in a table
    /// beside it. A projection maintained in parallel is a second source of
    /// truth that can disagree with the first, and the log is the one with the
    /// hash chain over it.
    pub fn sessions(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT payload, run_id, at FROM events WHERE event_type='session.opened' ORDER BY rowid",
        )?;
        let rows = statement.query_map([], |row| {
            let payload: String = row.get(0)?;
            Ok((
                payload,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut sessions: Vec<SessionSummary> = Vec::new();
        for row in rows {
            let (payload, run_id, at) = row?;
            let payload: serde_json::Value = serde_json::from_str(&payload)?;
            let Some(name) = payload["name"].as_str() else {
                continue;
            };
            let Some(run_id) = run_id.and_then(|value| uuid::Uuid::parse_str(&value).ok()) else {
                continue;
            };
            let at = chrono::DateTime::parse_from_rfc3339(&at)
                .map(|value| value.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| now());
            match sessions.iter_mut().find(|s| s.name == name) {
                Some(existing) => {
                    existing.runs.push(run_id);
                    existing.last_opened_at = at;
                }
                None => sessions.push(SessionSummary {
                    name: name.to_string(),
                    root: payload["root"].as_str().unwrap_or_default().to_string(),
                    runs: vec![run_id],
                    last_opened_at: at,
                }),
            }
        }
        Ok(sessions)
    }
    /// The runs of one session in order, empty if the name was never opened.
    pub fn session_runs(&self, name: &str) -> Result<Vec<Id>, StoreError> {
        Ok(self
            .sessions()?
            .into_iter()
            .find(|session| session.name == name)
            .map(|session| session.runs)
            .unwrap_or_default())
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
