use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::{AsyncTask, Env};
use napi::Task;
use napi_derive::napi;

use cf_core::{
    ContextEngine, ContextEntry, ContextStorage, CoreConfig, CoreError, EntryKind, EvictionPolicy,
    Result as CoreResult, ScoredEntry, Searcher,
};
use cf_storage::{open_storage, SqliteSearcher, SqliteStorage};

// ── JS-facing object types ──────────────────────────────────────────

#[napi(object)]
pub struct JsContextEntry {
    pub id: String,
    pub content: String,
    pub timestamp: i64,
    pub kind: String,
    pub token_count: Option<u32>,
}

#[napi(object)]
pub struct JsScoredEntry {
    pub entry: JsContextEntry,
    pub score: f64,
}

#[napi(object)]
pub struct JsConfig {
    pub max_entries: Option<u32>,
    pub token_budget: Option<u32>,
    pub eviction_policy: Option<String>,
}

// ── Conversion helpers ──────────────────────────────────────────────

fn kind_to_string(kind: &EntryKind) -> String {
    match kind {
        EntryKind::Manual => "manual".to_owned(),
        EntryKind::PreCompact => "pre_compact".to_owned(),
        EntryKind::Auto => "auto".to_owned(),
    }
}

fn parse_kind(s: &str) -> napi::Result<EntryKind> {
    match s {
        "manual" => Ok(EntryKind::Manual),
        "pre_compact" => Ok(EntryKind::PreCompact),
        "auto" => Ok(EntryKind::Auto),
        other => Err(napi::Error::new(
            napi::Status::InvalidArg,
            format!("unknown kind: '{other}'. Expected: manual, pre_compact, auto"),
        )),
    }
}

fn parse_eviction_policy(s: &str) -> napi::Result<EvictionPolicy> {
    match s {
        "lru" => Ok(EvictionPolicy::Lru),
        "least_relevant" => Ok(EvictionPolicy::LeastRelevant),
        other => Err(napi::Error::new(
            napi::Status::InvalidArg,
            format!("unknown eviction_policy: '{other}'. Expected: lru, least_relevant"),
        )),
    }
}

fn to_js_entry(e: ContextEntry) -> napi::Result<JsContextEntry> {
    let token_count = e
        .token_count
        .map(|v| {
            u32::try_from(v).map_err(|_| {
                napi::Error::new(
                    napi::Status::GenericFailure,
                    format!("token_count {v} overflows u32"),
                )
            })
        })
        .transpose()?;
    Ok(JsContextEntry {
        id: e.id,
        content: e.content,
        timestamp: e.timestamp,
        kind: kind_to_string(&e.kind),
        token_count,
    })
}

fn to_js_scored(se: ScoredEntry) -> napi::Result<JsScoredEntry> {
    Ok(JsScoredEntry {
        entry: to_js_entry(se.entry)?,
        score: se.score,
    })
}

fn core_err(e: CoreError) -> napi::Error {
    napi::Error::new(napi::Status::GenericFailure, e.to_string())
}

// ── Shared wrappers (delegate through Arc) ──────────────────────────

struct SharedStorage(Arc<SqliteStorage>);

impl ContextStorage for SharedStorage {
    fn save(&self, entry: &ContextEntry) -> CoreResult<()> {
        self.0.save(entry)
    }

    fn get_top_k(&self, k: usize) -> CoreResult<Vec<ContextEntry>> {
        self.0.get_top_k(k)
    }

    fn get_all(&self) -> CoreResult<Vec<ContextEntry>> {
        self.0.get_all()
    }

    fn delete(&self, id: &str) -> CoreResult<bool> {
        self.0.delete(id)
    }

    fn clear(&self) -> CoreResult<usize> {
        self.0.clear()
    }

    fn count(&self) -> CoreResult<usize> {
        self.0.count()
    }
}

struct SharedSearcher(Arc<SqliteSearcher>);

impl Searcher for SharedSearcher {
    fn search(&self, query: &str, limit: usize) -> CoreResult<Vec<ScoredEntry>> {
        self.0.search(query, limit)
    }
}

// ── Task definitions ────────────────────────────────────────────────

pub struct SaveTask {
    engine: Arc<ContextEngine>,
    content: String,
    kind: EntryKind,
}

impl Task for SaveTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.engine
            .save_snapshot(&self.content, self.kind.clone())
            .map_err(core_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct AssembleTask {
    engine: Arc<ContextEngine>,
    query: String,
    token_budget: usize,
}

impl Task for AssembleTask {
    type Output = Vec<ContextEntry>;
    type JsValue = Vec<JsContextEntry>;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.engine
            .assemble(&self.query, self.token_budget)
            .map_err(core_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        output.into_iter().map(to_js_entry).collect()
    }
}

pub struct SearchTask {
    searcher: Arc<SqliteSearcher>,
    query: String,
    limit: usize,
}

impl Task for SearchTask {
    type Output = Vec<ScoredEntry>;
    type JsValue = Vec<JsScoredEntry>;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.searcher
            .search(&self.query, self.limit)
            .map_err(core_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        output.into_iter().map(to_js_scored).collect()
    }
}

pub struct CountTask {
    storage: Arc<SqliteStorage>,
}

impl Task for CountTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.storage.count().map_err(core_err).and_then(|n| {
            u32::try_from(n).map_err(|_| {
                napi::Error::new(
                    napi::Status::GenericFailure,
                    format!("count {n} overflows u32"),
                )
            })
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ClearTask {
    storage: Arc<SqliteStorage>,
}

impl Task for ClearTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.storage.clear().map_err(core_err).and_then(|n| {
            u32::try_from(n).map_err(|_| {
                napi::Error::new(
                    napi::Status::GenericFailure,
                    format!("clear count {n} overflows u32"),
                )
            })
        })
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct DeleteTask {
    storage: Arc<SqliteStorage>,
    id: String,
}

impl Task for DeleteTask {
    type Output = bool;
    type JsValue = bool;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.storage.delete(&self.id).map_err(core_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct GetAllTask {
    storage: Arc<SqliteStorage>,
}

impl Task for GetAllTask {
    type Output = Vec<ContextEntry>;
    type JsValue = Vec<JsContextEntry>;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        self.storage.get_all().map_err(core_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        output.into_iter().map(to_js_entry).collect()
    }
}

// ── Main napi class ─────────────────────────────────────────────────

#[napi]
pub struct ContextForgeCore {
    engine: Arc<ContextEngine>,
    storage: Arc<SqliteStorage>,
    searcher: Arc<SqliteSearcher>,
    token_budget: usize,
}

#[napi]
impl ContextForgeCore {
    #[napi(constructor)]
    pub fn new(db_path: String, config: Option<JsConfig>) -> napi::Result<Self> {
        let max_entries = config.as_ref().and_then(|c| c.max_entries).unwrap_or(1000) as usize;
        let token_budget = config.as_ref().and_then(|c| c.token_budget).unwrap_or(4096) as usize;
        let eviction_policy = config
            .as_ref()
            .and_then(|c| c.eviction_policy.as_deref())
            .map(parse_eviction_policy)
            .transpose()?
            .unwrap_or(EvictionPolicy::Lru);

        let path = PathBuf::from(&db_path);

        let (sqlite_storage, sqlite_searcher) =
            open_storage(&path, max_entries).map_err(core_err)?;

        let storage = Arc::new(sqlite_storage);
        let searcher = Arc::new(sqlite_searcher);

        let core_config = CoreConfig {
            max_entries,
            token_budget,
            db_path: path,
            eviction_policy,
        };

        let engine = Arc::new(ContextEngine::new(
            Box::new(SharedStorage(Arc::clone(&storage))),
            Box::new(SharedSearcher(Arc::clone(&searcher))),
            core_config,
        ));

        Ok(Self {
            engine,
            storage,
            searcher,
            token_budget,
        })
    }

    #[napi]
    pub fn save(&self, content: String, kind: Option<String>) -> napi::Result<AsyncTask<SaveTask>> {
        let entry_kind = kind
            .as_deref()
            .map(parse_kind)
            .transpose()?
            .unwrap_or(EntryKind::Manual);
        Ok(AsyncTask::new(SaveTask {
            engine: Arc::clone(&self.engine),
            content,
            kind: entry_kind,
        }))
    }

    #[napi]
    pub fn assemble(&self, query: String, token_budget: Option<u32>) -> AsyncTask<AssembleTask> {
        let budget = token_budget
            .map(|b| b as usize)
            .unwrap_or(self.token_budget);
        AsyncTask::new(AssembleTask {
            engine: Arc::clone(&self.engine),
            query,
            token_budget: budget,
        })
    }

    #[napi]
    pub fn search(&self, query: String, limit: Option<u32>) -> AsyncTask<SearchTask> {
        let search_limit = limit.map(|l| l as usize).unwrap_or(10);
        AsyncTask::new(SearchTask {
            searcher: Arc::clone(&self.searcher),
            query,
            limit: search_limit,
        })
    }

    #[napi]
    pub fn count(&self) -> AsyncTask<CountTask> {
        AsyncTask::new(CountTask {
            storage: Arc::clone(&self.storage),
        })
    }

    #[napi]
    pub fn clear(&self) -> AsyncTask<ClearTask> {
        AsyncTask::new(ClearTask {
            storage: Arc::clone(&self.storage),
        })
    }

    #[napi]
    pub fn delete(&self, id: String) -> AsyncTask<DeleteTask> {
        AsyncTask::new(DeleteTask {
            storage: Arc::clone(&self.storage),
            id,
        })
    }

    #[napi]
    pub fn get_all(&self) -> AsyncTask<GetAllTask> {
        AsyncTask::new(GetAllTask {
            storage: Arc::clone(&self.storage),
        })
    }

    #[napi]
    pub fn close(&self) -> napi::Result<()> {
        let conn = self
            .storage
            .pool()
            .get()
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, e.to_string()))?;
        Ok(())
    }
}
