use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

use crate::event::{ActorType, Event, Kind};
use crate::record::Record;

// The cache: SQLite with FTS5. Disposable, rebuilt from the record.
pub struct Cache {
    conn: Connection,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Claim {
    pub id: String,
    pub tx: String,
    pub scope: String,
    pub actor_type: String,
    pub session: Option<String>,
    pub origin: Option<String>,
    pub src: Option<String>,
    pub text: String,
    pub trust: String,
    pub retracted: bool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS claims (
  id TEXT PRIMARY KEY,
  tx TEXT NOT NULL,
  scope TEXT NOT NULL,
  actor_type TEXT NOT NULL,
  session TEXT,
  origin TEXT,
  src TEXT,
  text TEXT NOT NULL,
  trust TEXT NOT NULL,
  retracted_at TEXT
);
CREATE INDEX IF NOT EXISTS claims_scope ON claims(scope, trust);
CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(text, content='claims', content_rowid='rowid');
CREATE TABLE IF NOT EXISTS history (
  ev_id TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  tx TEXT NOT NULL,
  kind TEXT NOT NULL,
  actor_type TEXT NOT NULL,
  session TEXT,
  reason TEXT
);
CREATE INDEX IF NOT EXISTS history_target ON history(target);
CREATE TABLE IF NOT EXISTS meta (
  file TEXT PRIMARY KEY,
  offset INTEGER NOT NULL
);
";

impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    // Applies every event the cache has not seen yet. Safe to call from
    // several processes: the write transaction serializes them and the
    // offsets are re-read inside it.
    pub fn catch_up(&mut self, record: &Record) -> Result<usize> {
        let txn = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut applied = 0;
        for path in record.log_files()? {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let offset: u64 = txn
                .query_row(
                    "SELECT offset FROM meta WHERE file = ?1",
                    params![name],
                    |r| r.get::<_, i64>(0),
                )
                .map(|v| v as u64)
                .unwrap_or(0);
            let (events, new_offset) = record.read_from(&path, offset)?;
            if new_offset == offset {
                continue;
            }
            for ev in &events {
                apply(&txn, ev)?;
                applied += 1;
            }
            txn.execute(
                "INSERT INTO meta(file, offset) VALUES (?1, ?2)
                 ON CONFLICT(file) DO UPDATE SET offset = excluded.offset",
                params![name, new_offset as i64],
            )?;
        }
        txn.commit()?;
        Ok(applied)
    }

    // Drops all derived rows and replays the record. The file stays in place,
    // so other processes with the cache open are not disturbed.
    pub fn rebuild(&mut self, record: &Record) -> Result<usize> {
        {
            let txn = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            txn.execute_batch(
                "INSERT INTO claims_fts(claims_fts) VALUES('delete-all');
                 DELETE FROM claims; DELETE FROM history; DELETE FROM meta;",
            )?;
            txn.commit()?;
        }
        self.catch_up(record)
    }

    pub fn get(&self, id: &str) -> Result<Option<Claim>> {
        let mut st = self.conn.prepare(
            "SELECT id, tx, scope, actor_type, session, origin, src, text, trust, retracted_at
             FROM claims WHERE id = ?1",
        )?;
        let mut rows = st.query(params![id])?;
        match rows.next()? {
            Some(r) => Ok(Some(row_to_claim(r)?)),
            None => Ok(None),
        }
    }

    pub fn history(&self, id: &str) -> Result<Vec<(String, String, String, String, Option<String>, Option<String>)>> {
        let mut st = self.conn.prepare(
            "SELECT ev_id, tx, kind, actor_type, session, reason FROM history
             WHERE target = ?1 ORDER BY tx",
        )?;
        let rows = st.query_map(params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // Search within the given scopes. Trusted first, then quarantine. Retracted excluded.
    pub fn search(&self, scopes: &[String], query: &str, limit: usize) -> Result<Vec<Claim>> {
        let scope_list = scopes
            .iter()
            .map(|s| format!("'{}'", s.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let sql = if query.trim().is_empty() {
            format!(
                "SELECT id, tx, scope, actor_type, session, origin, src, text, trust, retracted_at
                 FROM claims WHERE scope IN ({}) AND retracted_at IS NULL
                 ORDER BY CASE trust WHEN 'trusted' THEN 0 ELSE 1 END, tx DESC LIMIT {}",
                scope_list, limit
            )
        } else {
            format!(
                "SELECT c.id, c.tx, c.scope, c.actor_type, c.session, c.origin, c.src, c.text, c.trust, c.retracted_at
                 FROM claims_fts f JOIN claims c ON c.rowid = f.rowid
                 WHERE claims_fts MATCH ?1 AND c.scope IN ({}) AND c.retracted_at IS NULL
                 ORDER BY CASE c.trust WHEN 'trusted' THEN 0 ELSE 1 END, bm25(claims_fts) LIMIT {}",
                scope_list, limit
            )
        };
        let mut st = self.conn.prepare(&sql)?;
        let mut out = Vec::new();
        if query.trim().is_empty() {
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                out.push(row_to_claim(r)?);
            }
        } else {
            let mut rows = st.query(params![fts_query(query)])?;
            while let Some(r) = rows.next()? {
                out.push(row_to_claim(r)?);
            }
        }
        Ok(out)
    }
}

// Turns free text into a safe FTS5 query: each word quoted, prefix match, OR-joined.
fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn row_to_claim(r: &rusqlite::Row) -> rusqlite::Result<Claim> {
    let retracted_at: Option<String> = r.get(9)?;
    Ok(Claim {
        id: r.get(0)?,
        tx: r.get(1)?,
        scope: r.get(2)?,
        actor_type: r.get(3)?,
        session: r.get(4)?,
        origin: r.get(5)?,
        src: r.get(6)?,
        text: r.get(7)?,
        trust: r.get(8)?,
        retracted: retracted_at.is_some(),
    })
}

fn actor_str(a: &ActorType) -> &'static str {
    match a {
        ActorType::Human => "human",
        ActorType::Agent => "agent",
    }
}

fn apply(conn: &Connection, ev: &Event) -> Result<()> {
    match ev.kind {
        Kind::Claim => {
            // A human's own statements are trusted on entry; an agent's go to quarantine.
            let trust = match ev.actor.kind {
                ActorType::Human => "trusted",
                ActorType::Agent => "quarantine",
            };
            conn.execute(
                "INSERT OR IGNORE INTO claims
                 (id, tx, scope, actor_type, session, origin, src, text, trust)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    ev.id,
                    ev.tx,
                    ev.scope,
                    actor_str(&ev.actor.kind),
                    ev.actor.session,
                    ev.actor.origin,
                    ev.src,
                    ev.text.clone().unwrap_or_default(),
                    trust
                ],
            )?;
            conn.execute(
                "INSERT INTO claims_fts(rowid, text)
                 SELECT rowid, text FROM claims WHERE id = ?1",
                params![ev.id],
            )?;
        }
        Kind::Lift => {
            // Only a human can lift a claim into the trusted context.
            if ev.actor.kind == ActorType::Human {
                if let Some(t) = &ev.target {
                    conn.execute(
                        "UPDATE claims SET trust = 'trusted' WHERE id = ?1",
                        params![t],
                    )?;
                }
            }
            record_history(conn, ev)?;
        }
        Kind::Retract => {
            if let Some(t) = &ev.target {
                conn.execute(
                    "UPDATE claims SET retracted_at = ?2 WHERE id = ?1 AND retracted_at IS NULL",
                    params![t, ev.tx],
                )?;
            }
            record_history(conn, ev)?;
        }
        Kind::Episode => {}
    }
    Ok(())
}

fn record_history(conn: &Connection, ev: &Event) -> Result<()> {
    let kind = match ev.kind {
        Kind::Lift => "lift",
        Kind::Retract => "retract",
        _ => return Ok(()),
    };
    conn.execute(
        "INSERT OR IGNORE INTO history (ev_id, target, tx, kind, actor_type, session, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            ev.id,
            ev.target.clone().unwrap_or_default(),
            ev.tx,
            kind,
            actor_str(&ev.actor.kind),
            ev.actor.session,
            ev.reason
        ],
    )?;
    Ok(())
}
