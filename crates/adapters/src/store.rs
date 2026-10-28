use inspector_application::{HistoryTarget, Settings};
use inspector_domain::{Draft, Exchange};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};

pub struct Store {
    pub root: PathBuf,
    db: Connection,
    body_bytes: Cell<u64>,
}
type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("bodies")).map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(err)?;
        }
        let db = Connection::open(root.join("inspector.sqlite")).map_err(err)?;
        db.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(err)?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version > 3 {
            return Err(format!(
                "Database schema {version} requires a newer Inspector"
            ));
        }
        if version == 0 {
            db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")
                .map_err(err)?;
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS exchanges(id TEXT PRIMARY KEY,started INTEGER NOT NULL,pinned INTEGER NOT NULL DEFAULT 0,search TEXT NOT NULL,data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS exchange_time ON exchanges(started DESC);
            CREATE TABLE IF NOT EXISTS bodies(id TEXT PRIMARY KEY,size INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS exchange_bodies(exchange_id TEXT NOT NULL REFERENCES exchanges(id) ON DELETE CASCADE,body_id TEXT NOT NULL,PRIMARY KEY(exchange_id,body_id));
            CREATE TABLE IF NOT EXISTS drafts(id TEXT PRIMARY KEY,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS settings(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS cleared_exchanges(id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS repeater_results(draft_id TEXT PRIMARY KEY REFERENCES drafts(id) ON DELETE CASCADE,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS repeater_bodies(draft_id TEXT NOT NULL REFERENCES repeater_results(draft_id) ON DELETE CASCADE,body_id TEXT NOT NULL,PRIMARY KEY(draft_id,body_id));
            PRAGMA user_version=3;").map_err(err)?;
        let body_bytes: i64 = db
            .query_row("SELECT COALESCE(SUM(size),0) FROM bodies", [], |r| r.get(0))
            .map_err(err)?;
        let store = Self {
            root,
            db,
            body_bytes: Cell::new(body_bytes as u64),
        };
        store.cleanup_orphans()?;
        Ok(store)
    }
    fn body_path(&self, id: &str) -> PathBuf {
        let digest = Sha256::digest(id.as_bytes());
        self.root.join("bodies").join(format!("{digest:x}"))
    }
    pub fn put(&self, exchange: &Exchange) -> Result<bool> {
        if exchange.repeater_id.is_some() {
            return self.put_repeater(exchange);
        }
        // Live updates and reconnects must not restore explicitly cleared items.
        if self
            .db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cleared_exchanges WHERE id=?1)",
                [&exchange.id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(err)?
        {
            return Ok(false);
        }
        let text = serde_json::to_string(exchange).map_err(err)?;
        let search = format!(
            "{} {} {}",
            exchange.request.method, exchange.request.url, exchange.client_id
        )
        .to_lowercase();
        self.db
            .execute(
                "INSERT INTO exchanges(id,started,pinned,search,data) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(id) DO UPDATE SET data=excluded.data,search=excluded.search",
                params![
                    exchange.id,
                    exchange.started_at,
                    exchange.pinned,
                    search,
                    text
                ],
            )
            .map_err(err)?;
        for body in [
            exchange.request.body.as_ref(),
            exchange.response.as_ref().and_then(|r| r.body.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            self.db
                .execute(
                    "INSERT OR IGNORE INTO exchange_bodies VALUES(?1,?2)",
                    params![exchange.id, body.id],
                )
                .map_err(err)?;
        }
        Ok(true)
    }
    /// Reserve a new send before starting I/O. Late updates from older sends
    /// cannot overwrite this tab's current result, or revive a deleted tab.
    pub fn begin_repeater(&self, draft: &Draft, exchange: &Exchange) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(err)?;
        tx.execute(
            "INSERT INTO drafts VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![draft.id, serde_json::to_string(draft).map_err(err)?],
        )
        .map_err(err)?;
        tx.execute(
            "DELETE FROM repeater_results WHERE draft_id=?1",
            [&draft.id],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO repeater_results VALUES(?1,?2)",
            params![draft.id, serde_json::to_string(exchange).map_err(err)?],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        self.put_repeater(exchange)?;
        self.cleanup_orphans()
    }
    fn put_repeater(&self, exchange: &Exchange) -> Result<bool> {
        let id = exchange
            .repeater_id
            .as_deref()
            .ok_or("Missing Repeater owner")?;
        // Delayed bridge events cannot revive a cancelled run or roll a completed
        // response back to a pending request snapshot.
        let changed = self
            .db
            .execute(
                "UPDATE repeater_results SET data=?2
             WHERE draft_id=?1 AND json_extract(data,'$.id')=?3
             AND json_extract(data,'$.state')!='cancelled'
             AND (json_extract(?2,'$.state') NOT IN ('pending','held_request','held_response')
                  OR json_extract(data,'$.state') IN ('pending','held_request','held_response'))",
                params![
                    id,
                    serde_json::to_string(exchange).map_err(err)?,
                    exchange.id
                ],
            )
            .map_err(err)?;
        if changed == 0 {
            return Ok(false);
        }
        for body in [
            exchange.request.body.as_ref(),
            exchange.response.as_ref().and_then(|r| r.body.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            self.db
                .execute(
                    "INSERT OR IGNORE INTO repeater_bodies VALUES(?1,?2)",
                    params![id, body.id],
                )
                .map_err(err)?;
        }
        Ok(true)
    }
    pub fn repeater_result(&self, id: &str) -> Result<Option<Exchange>> {
        let text: Option<String> = self
            .db
            .query_row(
                "SELECT data FROM repeater_results WHERE draft_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        text.map(|s| serde_json::from_str(&s).map_err(err))
            .transpose()
    }
    pub fn clear_history(&self, target: &HistoryTarget) -> Result<usize> {
        if matches!(target, HistoryTarget::Items(ids) if ids.is_empty()) {
            return Ok(0);
        }
        let transaction = self.db.unchecked_transaction().map_err(err)?;
        let removed = match target {
            HistoryTarget::All => {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO cleared_exchanges SELECT id FROM exchanges",
                        [],
                    )
                    .map_err(err)?;
                transaction
                    .execute("DELETE FROM exchanges", [])
                    .map_err(err)?
            }
            HistoryTarget::Items(ids) => {
                let mut remember = transaction.prepare("INSERT OR IGNORE INTO cleared_exchanges SELECT id FROM exchanges WHERE id=?1").map_err(err)?;
                let mut delete = transaction
                    .prepare("DELETE FROM exchanges WHERE id=?1")
                    .map_err(err)?;
                let mut removed = 0;
                for id in ids {
                    remember.execute([id]).map_err(err)?;
                    removed += delete.execute([id]).map_err(err)?;
                }
                removed
            }
        };
        transaction.commit().map_err(err)?;
        self.cleanup_orphans()?;
        Ok(removed)
    }
    pub fn body_referenced(&self, id: &str) -> Result<bool> {
        self.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM exchange_bodies WHERE body_id=?1 UNION ALL SELECT 1 FROM repeater_bodies WHERE body_id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(err)
    }
    pub fn history(&self, query: &str, offset: usize) -> Result<(Vec<Exchange>, usize)> {
        let mut filters = vec![];
        let mut args = vec![];
        for term in query.split_whitespace() {
            let term = term.to_lowercase();
            if let Some(method) = term.strip_prefix("m:") {
                filters.push("lower(json_extract(data,'$.request.method'))=?");
                args.push(method.to_owned());
            } else if let Some(status) = term.strip_prefix("s:") {
                filters.push("instr(CAST(json_extract(data,'$.response.status') AS TEXT),?)=1");
                args.push(status.to_owned());
            } else {
                filters.push("instr(search,?)>0");
                args.push(term);
            }
        }
        let where_sql = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        let total: i64 = self
            .db
            .query_row(
                &format!("SELECT count(*) FROM exchanges{where_sql}"),
                rusqlite::params_from_iter(&args),
                |r| r.get(0),
            )
            .map_err(err)?;
        let mut stmt=self.db.prepare(&format!("SELECT data,pinned FROM exchanges{where_sql} ORDER BY started DESC,id LIMIT 200 OFFSET {}",offset.min(i64::MAX as usize))).map_err(err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(&args), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            })
            .map_err(err)?;
        let mut result = Vec::new();
        for row in rows {
            let (data, pinned) = row.map_err(err)?;
            let mut exchange: Exchange = serde_json::from_str(&data).map_err(err)?;
            exchange.pinned = pinned;
            result.push(exchange);
        }
        Ok((result, total as usize))
    }
    pub fn visit(&self, mut visitor: impl FnMut(Exchange) -> Result<()>) -> Result<()> {
        let mut stmt = self
            .db
            .prepare("SELECT data,pinned FROM exchanges ORDER BY started")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))
            .map_err(err)?;
        for r in rows {
            let (data, pinned) = r.map_err(err)?;
            let mut e: Exchange = serde_json::from_str(&data).map_err(err)?;
            e.pinned = pinned;
            visitor(e)?;
        }
        Ok(())
    }
    pub fn pin(&self, id: &str, pinned: bool) -> Result<()> {
        self.db
            .execute(
                "UPDATE exchanges SET pinned=?2 WHERE id=?1",
                params![id, pinned],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn body_exists(&self, id: &str) -> bool {
        self.body_path(id).is_file()
    }
    pub fn body(&self, id: &str) -> Result<Vec<u8>> {
        fs::read(self.body_path(id)).map_err(|e| format!("Body unavailable: {e}"))
    }
    pub fn put_body(&self, id: &str, bytes: &[u8]) -> Result<bool> {
        // A download can finish after its exchange was cleared.
        if !self.body_referenced(id)? {
            return Ok(false);
        }
        let mut settings = self.settings()?;
        if bytes.len() as u64 >= settings.quota_bytes {
            return Err("Body exceeds the storage quota; increase it in Settings".into());
        }
        let old_size: i64 = self
            .db
            .query_row("SELECT size FROM bodies WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(err)?
            .unwrap_or(0);
        let extra = (bytes.len() as u64).saturating_sub(old_size as u64);
        let db_size = fs::metadata(self.root.join("inspector.sqlite"))
            .map(|m| m.len())
            .unwrap_or(0);
        if self.body_bytes.get() + db_size + extra > settings.quota_bytes {
            settings.quota_bytes -= extra;
            if !self.prune_excluding(&settings, Some(id))? {
                return Err(
                    "Storage quota is full of protected data; increase it in Settings".into(),
                );
            }
        }
        let path = self.body_path(id);
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        fs::write(&temporary, bytes).map_err(err)?;
        fs::rename(&temporary, &path).map_err(err)?;
        self.db
            .execute(
                "INSERT INTO bodies VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET size=excluded.size",
                params![id, bytes.len() as i64],
            )
            .map_err(err)?;
        self.body_bytes
            .set(self.body_bytes.get().saturating_sub(old_size as u64) + bytes.len() as u64);
        Ok(true)
    }
    pub fn save_draft(&self, draft: &Draft) -> Result<()> {
        self.db
            .execute(
                "INSERT INTO drafts VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![draft.id, serde_json::to_string(draft).map_err(err)?],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn delete_draft(&self, id: &str) -> Result<()> {
        self.db
            .execute("DELETE FROM drafts WHERE id=?1", [id])
            .map_err(err)?;
        self.cleanup_orphans()
    }
    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let mut stmt = self
            .db
            .prepare("SELECT data FROM drafts ORDER BY rowid")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
            .collect()
    }
    pub fn settings(&self) -> Result<Settings> {
        match self
            .db
            .query_row("SELECT data FROM settings WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            }) {
            Ok(s) => serde_json::from_str(&s).map_err(err),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Settings::default()),
            Err(e) => Err(err(e)),
        }
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        if settings.retention_days == 0 || settings.quota_bytes < 1024 * 1024 {
            return Err("Retention must be positive and quota at least 1 MiB".into());
        }
        self.db.execute("INSERT INTO settings VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",[serde_json::to_string(settings).map_err(err)?]).map_err(err)?;
        Ok(())
    }
    pub fn prune(&self, settings: &Settings) -> Result<bool> {
        self.prune_excluding(settings, None)
    }
    fn prune_excluding(&self, settings: &Settings, protected_body: Option<&str>) -> Result<bool> {
        let cutoff = now() - i64::from(settings.retention_days) * 86_400_000;
        self.db.execute("DELETE FROM exchanges WHERE pinned=0 AND started<?1 AND id NOT IN(SELECT exchange_id FROM exchange_bodies WHERE body_id=?2)",params![cutoff,protected_body]).map_err(err)?;
        self.cleanup_orphans()?;
        loop {
            let total: i64 = self
                .db
                .query_row("SELECT COALESCE(SUM(size),0) FROM bodies", [], |r| r.get(0))
                .map_err(err)?;
            let db_size = fs::metadata(self.root.join("inspector.sqlite"))
                .map(|m| m.len())
                .unwrap_or(0);
            if total as u64 + db_size <= settings.quota_bytes {
                return Ok(true);
            }
            let changed=self.db.execute("DELETE FROM exchanges WHERE id IN(SELECT id FROM exchanges WHERE pinned=0 AND id NOT IN(SELECT exchange_id FROM exchange_bodies WHERE body_id=?1) ORDER BY started LIMIT 100)",[protected_body]).map_err(err)?;
            self.cleanup_orphans()?;
            if changed == 0 {
                return Ok(false);
            }
            self.db
                .execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(err)?;
        }
    }
    fn cleanup_orphans(&self) -> Result<()> {
        let mut stmt=self.db.prepare("SELECT id FROM bodies WHERE NOT EXISTS(SELECT 1 FROM exchange_bodies WHERE body_id=bodies.id) AND NOT EXISTS(SELECT 1 FROM repeater_bodies WHERE body_id=bodies.id)").map_err(err)?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        for id in ids {
            let size: i64 = self
                .db
                .query_row("SELECT size FROM bodies WHERE id=?1", [&id], |r| r.get(0))
                .map_err(err)?;
            let _ = fs::remove_file(self.body_path(&id));
            self.db
                .execute("DELETE FROM bodies WHERE id=?1", [id])
                .map_err(err)?;
            self.body_bytes
                .set(self.body_bytes.get().saturating_sub(size as u64));
        }
        for entry in fs::read_dir(self.root.join("bodies")).map_err(err)? {
            let entry = entry.map_err(err)?;
            if entry.path().extension().is_some_and(|s| s == "tmp") {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}
pub fn data_directory() -> Result<PathBuf> {
    directories::ProjectDirs::from("dev", "FlutterInspector", "Flutter Inspector")
        .map(|d| d.data_local_dir().to_owned())
        .ok_or("Cannot locate application data directory".into())
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("inspector.tmp");
    fs::write(&temp, bytes).map_err(err)?;
    fs::rename(temp, path).map_err(err)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn survives_restart_and_keeps_pinned_history() {
        let dir = tempfile::tempdir().expect("dir");
        {
            let s = Store::open(dir.path().into()).expect("store");
            let e = Exchange {
                id: "one".into(),
                started_at: 1,
                pinned: true,
                ..Default::default()
            };
            s.put(&e).expect("put");
            s.save_draft(&Draft::default()).expect("draft");
            assert!(s.prune(&Settings::default()).expect("prune"));
        }
        let s = Store::open(dir.path().into()).expect("reopen");
        assert_eq!(s.history("", 0).expect("history").1, 1);
        assert_eq!(s.drafts().expect("drafts").len(), 1);
    }
    #[test]
    fn body_ids_cannot_escape_directory() {
        let dir = tempfile::tempdir().expect("dir");
        let s = Store::open(dir.path().into()).expect("store");
        s.put(&Exchange {
            id: "path-test".into(),
            request: inspector_domain::Request {
                body: Some(inspector_domain::Body {
                    id: "../../outside".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
        .expect("exchange");
        s.put_body("../../outside", b"secret").expect("body");
        assert_eq!(s.body("../../outside").expect("read"), b"secret");
        assert_eq!(
            s.body_path("../../outside").parent(),
            Some(dir.path().join("bodies").as_path())
        );
    }
}
