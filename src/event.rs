use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    Human,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    #[serde(rename = "type")]
    pub kind: ActorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claim,
    Lift,
    Retract,
    Episode,
}

// One line of the append-only record. Never edited, never deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub tx: String,
    pub kind: Kind,
    pub actor: Actor,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Event {
    pub fn new(kind: Kind, actor: Actor, scope: &str) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: format!("ev_{}_{}", now.format("%Y%m%dT%H%M%S%3f"), short_rand()),
            tx: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind,
            actor,
            scope: scope.to_string(),
            text: None,
            src: None,
            target: None,
            reason: None,
        }
    }
}

fn short_rand() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{:02x}{:04x}", pid & 0xff, nanos & 0xffff)
}
