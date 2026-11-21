use anyhow::{anyhow, Result};
use std::path::Path;

use crate::cache::{Cache, Claim};
use crate::event::{Actor, ActorType, Event, Kind};
use crate::record::Record;

pub struct Store {
    record: Record,
    cache: Cache,
}

#[derive(Debug, serde::Serialize)]
pub struct Explanation {
    pub claim: Claim,
    pub source_text: Option<String>,
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, serde::Serialize)]
pub struct HistoryEntry {
    pub event: String,
    pub tx: String,
    pub kind: String,
    pub actor: String,
    pub session: Option<String>,
    pub reason: Option<String>,
}

impl Store {
    // Opens the data dir and brings the shared cache up to date with the record.
    pub fn open(dir: &Path) -> Result<Self> {
        let record = Record::open(&dir.join("record"))?;
        std::fs::create_dir_all(dir.join("cache"))?;
        let mut cache = Cache::open(&dir.join("cache").join("index.sqlite"))?;
        cache.catch_up(&record)?;
        Ok(Self { record, cache })
    }

    pub fn rebuild(&mut self) -> Result<usize> {
        let _lock = self.record.lock()?;
        self.cache.rebuild(&self.record)
    }

    // Other processes may have written since our last call.
    fn sync(&mut self) -> Result<()> {
        self.cache.catch_up(&self.record)?;
        Ok(())
    }

    fn commit(&mut self, ev: &Event) -> Result<()> {
        let lock = self.record.lock()?;
        self.record.append(&lock, ev)?;
        let kind = serde_json::to_value(&ev.kind)?
            .as_str()
            .unwrap_or("event")
            .to_string();
        self.record
            .git_commit(&lock, &format!("{} {} {}", kind, ev.scope, ev.id));
        self.cache.catch_up(&self.record)?;
        Ok(())
    }

    pub fn claim(
        &mut self,
        actor: Actor,
        scope: &str,
        text: &str,
        source: Option<&str>,
    ) -> Result<String> {
        if text.trim().is_empty() {
            return Err(anyhow!("empty text"));
        }
        if scope != "global" && !scope.starts_with("project:") {
            return Err(anyhow!("scope must be global or project:<name>"));
        }
        let mut ev = Event::new(Kind::Claim, actor, scope);
        ev.text = Some(text.trim().to_string());
        if let Some(s) = source {
            ev.src = Some(self.record.put_source(s.as_bytes())?);
        }
        self.commit(&ev)?;
        Ok(ev.id)
    }

    pub fn lift(&mut self, id: &str, reason: Option<&str>) -> Result<()> {
        self.sync()?;
        let claim = self
            .cache
            .get(id)?
            .ok_or_else(|| anyhow!("unknown claim {}", id))?;
        let mut ev = Event::new(Kind::Lift, human(), &claim.scope);
        ev.target = Some(id.to_string());
        ev.reason = reason.map(|s| s.to_string());
        self.commit(&ev)
    }

    pub fn retract(&mut self, id: &str, actor: Actor, reason: Option<&str>) -> Result<()> {
        self.sync()?;
        let claim = self
            .cache
            .get(id)?
            .ok_or_else(|| anyhow!("unknown claim {}", id))?;
        let mut ev = Event::new(Kind::Retract, actor, &claim.scope);
        ev.target = Some(id.to_string());
        ev.reason = reason.map(|s| s.to_string());
        self.commit(&ev)
    }

    pub fn recall(&mut self, scope: &str, query: &str, limit: usize) -> Result<Vec<Claim>> {
        self.sync()?;
        let mut scopes = vec!["global".to_string()];
        if scope != "global" {
            scopes.push(scope.to_string());
        }
        self.cache.search(&scopes, query, limit)
    }

    pub fn explain(&mut self, id: &str) -> Result<Explanation> {
        self.sync()?;
        let claim = self
            .cache
            .get(id)?
            .ok_or_else(|| anyhow!("unknown claim {}", id))?;
        let source_text = match &claim.src {
            Some(h) => self
                .record
                .get_source(h)?
                .map(|b| String::from_utf8_lossy(&b).to_string()),
            None => None,
        };
        let history = self
            .cache
            .history(id)?
            .into_iter()
            .map(|(event, tx, kind, actor, session, reason)| HistoryEntry {
                event,
                tx,
                kind,
                actor,
                session,
                reason,
            })
            .collect();
        Ok(Explanation {
            claim,
            source_text,
            history,
        })
    }
}

pub fn human() -> Actor {
    Actor {
        kind: ActorType::Human,
        session: None,
        origin: Some("cli".to_string()),
    }
}

pub fn agent(session: Option<String>, origin: Option<String>) -> Actor {
    Actor {
        kind: ActorType::Agent,
        session,
        origin,
    }
}
