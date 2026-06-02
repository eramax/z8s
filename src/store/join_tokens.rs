use anyhow::{Context, Result, bail};
use base64::Engine;
use redb::ReadableTable;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::db::RedbBackend;

pub const JOIN_TOKEN_PREFIX: &str = "z8s.jt.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinTokenRecord {
    pub node_name: String,
    pub token_id: String,
    pub secret_hash: [u8; 32],
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub used_at_ms: Option<i64>,
    pub revoked: bool,
}

pub struct JoinIdentity {
    pub node_name: String,
    pub token_id: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn hash_secret(secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(secret);
    h.finalize().into()
}

pub fn verify_secret(expected_hash: &[u8; 32], secret: &[u8]) -> bool {
    let got = hash_secret(secret);
    constant_time_eq(expected_hash, &got)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

pub fn random_token_id() -> String {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("getrandom");
    hex::encode(buf)
}

fn random_secret() -> Vec<u8> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("getrandom");
    buf.to_vec()
}

/// Wire form: `z8s.jt.<token_id>.<secret_base64url>`
pub fn format_wire_token(token_id: &str, secret: &[u8]) -> String {
    format!(
        "{JOIN_TOKEN_PREFIX}{}.{}",
        token_id,
        URL_SAFE_NO_PAD.encode(secret)
    )
}

pub fn parse_wire_token(token: &str) -> Result<(String, Vec<u8>)> {
    let rest = token
        .strip_prefix(JOIN_TOKEN_PREFIX)
        .context("token must start with z8s.jt.")?;
    let (token_id, secret_b64) = rest
        .split_once('.')
        .context("token must be z8s.jt.<id>.<secret>")?;
    if token_id.is_empty() {
        bail!("empty token id");
    }
    let secret = URL_SAFE_NO_PAD
        .decode(secret_b64)
        .context("invalid token secret encoding")?;
    Ok((token_id.to_string(), secret))
}

pub fn parse_bearer(headers: &axum::http::HeaderMap) -> Result<String> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .context("missing Authorization header")?
        .to_str()
        .context("invalid Authorization header")?;
    auth.strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .map(|s| s.trim().to_string())
        .context("expected Bearer token")
}

pub fn new_join_record(node_name: &str) -> (JoinTokenRecord, Vec<u8>) {
    let secret = random_secret();
    let token_id = random_token_id();
    let record = JoinTokenRecord {
        node_name: node_name.to_string(),
        token_id: token_id.clone(),
        secret_hash: hash_secret(&secret),
        created_at_ms: now_ms(),
        expires_at_ms: None,
        used_at_ms: None,
        revoked: false,
    };
    (record, secret)
}

impl RedbBackend {
    pub async fn write_join_token(&self, record: &JoinTokenRecord) -> Result<()> {
        let db = self.db.clone();
        let key = record.node_name.clone();
        let bytes = serde_json::to_vec(record)?;
        tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let write_txn = db.begin_write()?;
            {
                let mut table = write_txn.open_table(crate::store::db::JOIN_TOKENS)?;
                table.insert(key.as_str(), bytes.as_slice())?;
            }
            write_txn.commit()?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    pub async fn read_join_token(&self, node_name: &str) -> Option<JoinTokenRecord> {
        let db = self.db.clone();
        let key = node_name.to_string();
        tokio::task::spawn_blocking(move || -> Option<JoinTokenRecord> {
            let read_txn = db.begin_read().ok()?;
            let table = read_txn.open_table(crate::store::db::JOIN_TOKENS).ok()?;
            let value = table.get(key.as_str()).ok()??;
            serde_json::from_slice(value.value()).ok()
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn read_join_token_by_id(&self, token_id: &str) -> Option<JoinTokenRecord> {
        let all = self.list_join_tokens().await;
        all.into_iter()
            .find(|r| r.token_id == token_id && !r.revoked)
    }

    pub async fn list_join_tokens(&self) -> Vec<JoinTokenRecord> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Vec<JoinTokenRecord> {
            let read_txn = match db.begin_read() {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let table = match read_txn.open_table(crate::store::db::JOIN_TOKENS) {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let mut out = Vec::new();
            let Ok(iter) = table.iter() else {
                return out;
            };
            for item in iter {
                let (_key, value) = match item {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
                if let Ok(rec) = serde_json::from_slice::<JoinTokenRecord>(value.value()) {
                    out.push(rec);
                }
            }
            out
        })
        .await
        .unwrap_or_default()
    }

    /// Create or return existing token. When `rotate` or missing, returns new wire secret.
    pub async fn ensure_join_token(
        &self,
        node_name: &str,
        rotate: bool,
    ) -> Result<Option<String>> {
        if rotate {
            let (record, secret) = new_join_record(node_name);
            self.write_join_token(&record).await?;
            return Ok(Some(format_wire_token(&record.token_id, &secret)));
        }
        if let Some(rec) = self.read_join_token(node_name).await {
            if !rec.revoked {
                return Ok(None);
            }
        }
        let (record, secret) = new_join_record(node_name);
        self.write_join_token(&record).await?;
        Ok(Some(format_wire_token(&record.token_id, &secret)))
    }

    pub async fn authenticate_join(
        &self,
        headers: &axum::http::HeaderMap,
        claimed_node_name: Option<&str>,
    ) -> Result<JoinIdentity> {
        let wire = parse_bearer(headers)?;
        let (token_id, secret) = parse_wire_token(&wire)?;
        let rec = self
            .read_join_token_by_id(&token_id)
            .await
            .context("unknown join token")?;
        if rec.revoked {
            bail!("join token revoked");
        }
        if let Some(exp) = rec.expires_at_ms {
            if now_ms() > exp {
                bail!("join token expired");
            }
        }
        if !verify_secret(&rec.secret_hash, &secret) {
            bail!("invalid join token secret");
        }
        if let Some(claimed) = claimed_node_name {
            if claimed != rec.node_name {
                bail!(
                    "token is for node '{}' but connection claimed '{}'",
                    rec.node_name,
                    claimed
                );
            }
        }
        let mut updated = rec.clone();
        if updated.used_at_ms.is_none() {
            updated.used_at_ms = Some(now_ms());
            self.write_join_token(&updated).await.ok();
        }
        Ok(JoinIdentity {
            node_name: rec.node_name,
            token_id: rec.token_id,
        })
    }
}

// Minimal hex without extra crate — use simple hex encode
mod hex {
    pub fn encode(bytes: [u8; 8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
