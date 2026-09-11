//! Operator HTTP API (`lapis api`). Face over the same Engine CLI/MCP use.
//!
//! `/v1` health/ready/vault/search/paths-search plus note/neighbors/tree-retrieve/
//! analytics/reindex. Every JSON body is `{ok, data, error, meta}`. RFC 7807
//! is not a second body.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, RawQuery, Request, State};
use axum::http::{StatusCode, header::AUTHORIZATION};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::limit::RequestBodyLimitLayer;

use crate::backend::Backend;
use crate::cli::ApiArgs;
use crate::config::DEFAULT_API_BIND;
use crate::envelope::{self, API_VERSION, Diagnostic, Envelope, ErrorBody, Meta, Recovery};
use crate::error::{LapisError, Result};
use crate::lattice::{self, Hit, Mode, PathHit, SearchParams, SearchResult};
use crate::notes;
use crate::ops::Ctx;

const FORENSIC_TREE: &str = "lapis.forensic.tree_retrieve.embedded";

#[derive(Clone)]
struct ApiState {
    ctx: Arc<Ctx>,
    backend: Backend,
    /// Some when bind is not loopback; requests must send `Authorization: Bearer`.
    token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindClass {
    Loopback,
    Private,
    Public,
}

/// Serve `/v1`. Loopback needs no token. Private/Tailscale bind requires
/// `LAPIS_API_TOKEN`. Public bind is out of scope.
pub async fn serve(ctx: Ctx, args: ApiArgs) -> Result<()> {
    let bind = args.bind.unwrap_or_else(|| ctx.cfg.api.bind.clone());
    let port = args.port.unwrap_or(ctx.cfg.api.port);
    let env_token = std::env::var("LAPIS_API_TOKEN").ok().filter(|s| !s.is_empty());
    let (addr, token) = bind_policy(&bind, port, env_token.as_deref())?;
    let backend = ctx.backend()?;
    let state = ApiState { ctx: Arc::new(ctx), backend, token };
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| LapisError::Internal(format!("bind {addr}: {e}")))?;
    eprintln!("lapis api listening on http://{addr}");
    axum::serve(listener, router(state)).await.map_err(|e| LapisError::Internal(format!("api: {e}")))
}

fn listen_addr(bind: &str, port: u16) -> Result<SocketAddr> {
    let host = if bind.eq_ignore_ascii_case("localhost") { DEFAULT_API_BIND } else { bind };
    let ip: IpAddr = host
        .parse()
        .map_err(|_| LapisError::Usage(format!("invalid api.bind {bind:?}; expected an IP address")))?;
    Ok(SocketAddr::new(ip, port))
}

fn bind_class(ip: IpAddr) -> BindClass {
    match ip {
        IpAddr::V4(v) if v.is_loopback() => BindClass::Loopback,
        IpAddr::V6(v) if v.is_loopback() => BindClass::Loopback,
        IpAddr::V4(v) if v.is_private() || v.is_link_local() || is_cgnat(v) => BindClass::Private,
        IpAddr::V6(v) if v.is_unique_local() || v.is_unicast_link_local() => BindClass::Private,
        _ => BindClass::Public,
    }
}

fn is_cgnat(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

fn bind_policy(bind: &str, port: u16, token: Option<&str>) -> Result<(SocketAddr, Option<String>)> {
    let addr = listen_addr(bind, port)?;
    match bind_class(addr.ip()) {
        BindClass::Loopback => Ok((addr, None)),
        BindClass::Public => Err(LapisError::Usage(
            "public bind is out of scope; api.bind must be loopback or a private/Tailscale address".into(),
        )),
        BindClass::Private => match token {
            Some(t) => Ok((addr, Some(t.to_string()))),
            None => Err(LapisError::Usage(
                "LAPIS_API_TOKEN required when api.bind is not loopback (set the env or bind 127.0.0.1)"
                    .into(),
            )),
        },
    }
}

async fn require_token(State(st): State<ApiState>, req: Request, next: Next) -> Response {
    let Some(expect) = st.token.as_deref() else {
        return next.run(req).await;
    };
    let got = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if got == Some(expect) {
        return next.run(req).await;
    }
    err_json(
        StatusCode::UNAUTHORIZED,
        ErrorBody {
            code: Some("unauthorized"),
            kind: "auth",
            message: "LAPIS_API_TOKEN required when api.bind is not loopback".into(),
            exit: 1,
            diagnostics: vec![],
            recovery: vec![
                Recovery {
                    action: "set_token",
                    detail: "set LAPIS_API_TOKEN (Halo secret store or env)".into(),
                    disk_inventory_allowed: None,
                },
                Recovery {
                    action: "bind_loopback",
                    detail: "default api.bind=127.0.0.1 does not require a token".into(),
                    disk_inventory_allowed: None,
                },
            ],
            miss_id: None,
        },
        Meta::default().with_api_version(),
    )
}

fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/ready", get(ready))
        .route("/v1/vault", get(vault_info))
        .route("/v1/search", post(search_handler))
        .route("/v1/paths/search", post(paths_search_handler))
        .route("/v1/notes/{path}/neighbors", get(neighbors_handler))
        .route("/v1/notes/{path}", get(note_handler))
        .route("/v1/tree-retrieve", post(tree_handler))
        .route("/v1/analytics/{name}", get(analytics_handler))
        .route("/v1/reindex", post(reindex_handler))
        .fallback(unknown)
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "default_true")]
    pub per_doc: bool,
}

fn default_search_limit() -> u32 {
    10
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsSearchRequest {
    pub pattern: String,
    #[serde(rename = "match", default)]
    pub match_kind: PathMatchWire,
    #[serde(default = "default_paths_limit")]
    pub limit: u32,
}

fn default_paths_limit() -> u32 {
    50
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathMatchWire {
    #[default]
    Basename,
    Glob,
    Substring,
}

impl From<PathMatchWire> for lapis_lattice::PathMatch {
    fn from(m: PathMatchWire) -> Self {
        match m {
            PathMatchWire::Basename => lapis_lattice::PathMatch::Basename,
            PathMatchWire::Glob => lapis_lattice::PathMatch::Glob,
            PathMatchWire::Substring => lapis_lattice::PathMatch::Substring,
        }
    }
}

#[derive(Serialize)]
struct HealthData {
    api_version: &'static str,
    lapis_version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyData {
    ready: bool,
    vault_root: String,
    backend: &'static str,
    source: String,
    reachable: bool,
}

#[derive(Serialize)]
pub(crate) struct PathsSearchData {
    pub count: usize,
    pub hits: Vec<PathHit>,
}

#[derive(Debug, Default)]
struct NoteQuery {
    heading: Option<String>,
    chunk: Option<usize>,
    max_chars: Option<usize>,
    meta_only: bool,
}

#[derive(Debug)]
struct NeighborsQuery {
    direction: String,
    dangling: bool,
    hop: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeRetrieveRequest {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default = "default_tree_depth")]
    pub depth: u32,
    #[serde(default = "default_tree_max_nodes")]
    pub max_nodes: u32,
}

fn default_tree_depth() -> u32 {
    2
}
fn default_tree_max_nodes() -> u32 {
    60
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReindexRequest {
    pub path: String,
}

#[derive(Serialize)]
struct NeighborsData {
    path: String,
    direction: String,
    hop: u32,
    neighbors: Vec<NeighborRow>,
}

#[derive(Serialize)]
struct NeighborRow {
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dst_raw: Option<String>,
    resolved: bool,
    direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<String>,
}

/// Outcome of a `/v1` operation (HTTP handler and golden tests).
pub enum ApiV1<T> {
    Ok { data: T, meta: Meta },
    Err { status: StatusCode, error: ErrorBody, meta: Meta },
}

/// Outcome of the shipped search entry point (HTTP handler and golden tests).
pub enum SearchV1 {
    Ok { data: SearchResult, meta: Meta },
    Err { status: StatusCode, error: ErrorBody, meta: Meta },
}

async fn health() -> Response {
    ok_json(
        HealthData { api_version: API_VERSION, lapis_version: env!("CARGO_PKG_VERSION") },
        Meta::default().with_api_version(),
    )
}

async fn ready(State(st): State<ApiState>) -> Response {
    let t0 = Instant::now();
    match st.backend.health().await {
        Ok(_) => ok_json(
            ReadyData {
                ready: true,
                vault_root: st.ctx.vault.root.display().to_string(),
                backend: st.backend.mode(),
                source: st.backend.source(),
                reachable: true,
            },
            Meta::default().with_latency(ms(t0)).with_api_version(),
        ),
        Err(e) => from_engine(&e, Meta::default().with_latency(ms(t0))),
    }
}

async fn vault_info(State(st): State<ApiState>) -> Response {
    let t0 = Instant::now();
    match crate::ops::vault_info(&st.ctx).await {
        Ok((info, _)) => ok_json(info, Meta::default().with_latency(ms(t0)).with_api_version()),
        Err(e) => from_engine(&e, Meta::default().with_latency(ms(t0))),
    }
}

async fn search_handler(State(st): State<ApiState>, body: Bytes) -> Response {
    let t0 = Instant::now();
    let req: SearchRequest = match decode_body(&body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    if let Some(resp) = validate_search(&req) {
        return resp;
    }
    match search_v1(&st.backend, req).await {
        SearchV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        SearchV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn paths_search_handler(State(st): State<ApiState>, body: Bytes) -> Response {
    let t0 = Instant::now();
    let req: PathsSearchRequest = match decode_body(&body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    if let Some(resp) = validate_paths(&req) {
        return resp;
    }
    match paths_search_v1(&st.backend, &req).await {
        PathsV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        PathsV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn note_handler(
    State(st): State<ApiState>,
    AxumPath(path): AxumPath<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let t0 = Instant::now();
    let q = match parse_note_query(raw.as_deref()) {
        Ok(q) => q,
        Err(resp) => return *resp,
    };
    match note_get_v1(&st.ctx.vault.root, &path, &q) {
        ApiV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        ApiV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn neighbors_handler(
    State(st): State<ApiState>,
    AxumPath(path): AxumPath<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let t0 = Instant::now();
    let q = match parse_neighbors_query(raw.as_deref()) {
        Ok(q) => q,
        Err(resp) => return *resp,
    };
    match neighbors_v1(&st.backend, &path, &q).await {
        ApiV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        ApiV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn tree_handler(State(st): State<ApiState>, body: Bytes) -> Response {
    let t0 = Instant::now();
    let req: TreeRetrieveRequest = match decode_body(&body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    if let Some(resp) = validate_tree(&req) {
        return resp;
    }
    match tree_retrieve_v1(&st.backend, req).await {
        ApiV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        ApiV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn analytics_handler(State(st): State<ApiState>, AxumPath(name): AxumPath<String>) -> Response {
    let t0 = Instant::now();
    match analytics_v1(&st.backend, &name).await {
        ApiV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        ApiV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn reindex_handler(State(st): State<ApiState>, body: Bytes) -> Response {
    let t0 = Instant::now();
    let req: ReindexRequest = match decode_body(&body) {
        Ok(r) => r,
        Err(resp) => return *resp,
    };
    if req.path.trim().is_empty() {
        return invalid_request("path is required");
    }
    match reindex_v1(&st.backend, &st.ctx.vault.root, req).await {
        ApiV1::Ok { data, meta } => ok_json(data, meta.with_latency(ms(t0)).with_api_version()),
        ApiV1::Err { status, error, meta } => {
            err_json(status, error, meta.with_latency(ms(t0)).with_api_version())
        }
    }
}

async fn unknown() -> Response {
    err_json(
        StatusCode::NOT_FOUND,
        ErrorBody {
            code: Some("invalid_request"),
            kind: "usage",
            message: "no such route".into(),
            exit: 1,
            diagnostics: vec![],
            recovery: vec![Recovery {
                action: "paths_search",
                detail: "use a documented /v1 operationId".into(),
                disk_inventory_allowed: None,
            }],
            miss_id: None,
        },
        Meta::default().with_api_version(),
    )
}

/// Shipped search entry point: content search, plus path-channel fuse when the
/// query is filename-like. Golden tests call this, not a private matcher.
pub async fn search_v1(backend: &Backend, req: SearchRequest) -> SearchV1 {
    let limit = req.limit;
    let offset = req.offset;
    let requested = offset + limit;
    let filename_like = is_filename_like(&req.query);
    let mut diagnostics_seed: Option<String> = None;
    let params = SearchParams {
        query: req.query.clone(),
        top_k: requested,
        domain: req.domain.clone(),
        mode: req.mode,
        per_doc: req.per_doc,
        mmr: false,
        include_archives: false,
        embedder: None,
    };

    let content = match backend.search(&params).await {
        Ok(r) => Some(r),
        Err(e) if filename_like => {
            diagnostics_seed = Some(e.to_string());
            None
        }
        Err(e) => {
            let (status, error) = map_engine(&e);
            return SearchV1::Err { status, error, meta: Meta::default() };
        }
    };

    let mut diagnostics = vec![Diagnostic {
        fact: "filename_like".into(),
        value: json!(filename_like),
        evidence: Some("searchPost".into()),
    }];
    if let Some(msg) = diagnostics_seed {
        diagnostics.push(Diagnostic {
            fact: "content_search".into(),
            value: json!(msg),
            evidence: Some("searchPost".into()),
        });
    }

    if filename_like {
        let match_kind = path_match_for_query(&req.query);
        match backend.paths_search(&req.query, match_kind, requested.max(1)).await {
            Ok(paths) => {
                diagnostics.push(Diagnostic {
                    fact: "path_channel".into(),
                    value: json!(true),
                    evidence: Some("pathsSearchPost".into()),
                });
                let mut data = content.unwrap_or_else(|| empty_search(&req));
                data = fuse_paths(data, paths, requested);
                let data = page_search(data, limit, offset);
                let mut meta = search_meta(&data, limit, offset);
                meta.search_capable = Some(true);
                meta.diagnostics = diagnostics;
                return SearchV1::Ok { data, meta };
            }
            Err(e) => {
                return SearchV1::Err {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    error: index_gap_error(&e),
                    meta: Meta { search_capable: Some(false), diagnostics, ..Meta::default() },
                };
            }
        }
    }

    let mut data = content.unwrap_or_else(|| empty_search(&req));
    data = page_search(data, limit, offset);
    diagnostics.push(Diagnostic {
        fact: "path_channel".into(),
        value: json!(false),
        evidence: Some("not filename-like".into()),
    });
    let mut meta = search_meta(&data, limit, offset);
    meta.search_capable = Some(true);
    meta.diagnostics = diagnostics;
    SearchV1::Ok { data, meta }
}

pub(crate) enum PathsV1 {
    Ok { data: PathsSearchData, meta: Meta },
    Err { status: StatusCode, error: ErrorBody, meta: Meta },
}

pub(crate) async fn paths_search_v1(backend: &Backend, req: &PathsSearchRequest) -> PathsV1 {
    match backend.paths_search(&req.pattern, req.match_kind.into(), req.limit).await {
        Ok(hits) => {
            let count = hits.len();
            let truncated = req.limit > 0 && count as u32 >= req.limit;
            let meta = Meta {
                truncated,
                next: None,
                count: Some(count),
                limit: Some(req.limit),
                search_capable: Some(true),
                diagnostics: vec![Diagnostic {
                    fact: "path_channel".into(),
                    value: json!(true),
                    evidence: Some("pathsSearchPost".into()),
                }],
                ..Meta::default()
            };
            PathsV1::Ok { data: PathsSearchData { count, hits }, meta }
        }
        Err(e) => PathsV1::Err {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error: index_gap_error(&e),
            meta: Meta { search_capable: Some(false), ..Meta::default() },
        },
    }
}

/// Shipped noteGet entry point.
fn note_get_v1(root: &Path, path: &str, q: &NoteQuery) -> ApiV1<notes::Note> {
    let path = percent_decode(path);
    let mut note = match notes::read(root, &path) {
        Ok(n) => n,
        Err(e) => return engine_api(e),
    };
    if q.meta_only {
        note.body.clear();
        return ApiV1::Ok { data: note, meta: Meta::default() };
    }
    match notes::excerpt(&note, q.heading.as_deref(), q.chunk, q.max_chars) {
        Ok((body, truncated)) => {
            note.body = body;
            ApiV1::Ok { data: note, meta: Meta { truncated, ..Meta::default() } }
        }
        Err(e) => engine_api(e),
    }
}

/// Shipped neighborsGet entry point. Hop-2 on embedded is a typed error, never silent empty.
async fn neighbors_v1(backend: &Backend, path: &str, q: &NeighborsQuery) -> ApiV1<NeighborsData> {
    let rel = match clean_note_path(path) {
        Ok(r) => r,
        Err(e) => return engine_api(e),
    };
    if q.hop == 2 && backend.mode() == "embedded" {
        return ApiV1::Err {
            status: StatusCode::CONFLICT,
            error: mode_unsupported("`neighborsGet` hop=2 (the ego graph)", None),
            meta: Meta::default(),
        };
    }
    let resolved_only = !q.dangling;
    if q.hop == 2 {
        return match backend.ego(&rel, 2, &q.direction, resolved_only).await {
            Ok(e) => {
                let neighbors: Vec<NeighborRow> = e
                    .rows
                    .into_iter()
                    .map(|r| NeighborRow {
                        path: r.path,
                        dst_raw: r.dst_raw,
                        resolved: r.resolved,
                        direction: r.direction,
                        depth: Some(r.depth),
                        via: r.via,
                    })
                    .collect();
                ApiV1::Ok {
                    data: NeighborsData { path: e.path, direction: e.direction, hop: 2, neighbors },
                    meta: Meta { truncated: e.truncated, count: Some(e.count), ..Meta::default() },
                }
            }
            Err(e) => engine_api(e),
        };
    }
    match backend.neighbors(&rel, &q.direction, resolved_only).await {
        Ok(n) => {
            let neighbors: Vec<NeighborRow> = n
                .neighbors
                .into_iter()
                .map(|r| NeighborRow {
                    path: r.path,
                    dst_raw: r.dst_raw,
                    resolved: r.resolved,
                    direction: r.direction,
                    depth: Some(1),
                    via: None,
                })
                .collect();
            ApiV1::Ok {
                data: NeighborsData { path: n.path, direction: n.direction, hop: 1, neighbors },
                meta: Meta::default(),
            }
        }
        Err(e) => engine_api(e),
    }
}

/// Shipped treeRetrievePost entry point. Embedded is a forensic miss, never a string-only usage.
pub(crate) async fn tree_retrieve_v1(backend: &Backend, req: TreeRetrieveRequest) -> ApiV1<lattice::Tree> {
    if backend.mode() == "embedded" {
        return ApiV1::Err {
            status: StatusCode::CONFLICT,
            error: mode_unsupported("tree-retrieve requires lattice.mode=http", Some(FORENSIC_TREE)),
            meta: Meta::default(),
        };
    }
    let rel = match req.path.as_deref() {
        Some(p) => match clean_note_path(p) {
            Ok(r) => Some(r),
            Err(e) => return engine_api(e),
        },
        None => None,
    };
    match backend.tree(rel.as_deref(), req.query.as_deref(), req.depth, req.max_nodes).await {
        Ok(t) => {
            let truncated = t.truncated;
            let count = t.count;
            ApiV1::Ok { data: t, meta: Meta { truncated, count: Some(count), ..Meta::default() } }
        }
        Err(e) => engine_api(e),
    }
}

/// Shipped analyticsGet entry point. Allowlist only; no raw SQL.
async fn analytics_v1(backend: &Backend, name: &str) -> ApiV1<lattice::Analytics> {
    if let Err(e) = lattice::check_analytics_query(name) {
        let (status, error) = map_engine(&e);
        return ApiV1::Err { status, error, meta: Meta::default() };
    }
    match backend.analytics(name).await {
        Ok(a) => {
            let truncated = a.truncated;
            let count = a.count;
            ApiV1::Ok { data: a, meta: Meta { truncated, count: Some(count), ..Meta::default() } }
        }
        Err(e) => engine_api(e),
    }
}

/// Shipped reindexPost entry point. Embedded sqlite only; never writes `lattice.db`.
async fn reindex_v1(backend: &Backend, vault: &Path, req: ReindexRequest) -> ApiV1<lattice::Reindex> {
    if backend.mode() == "http" {
        return ApiV1::Err {
            status: StatusCode::CONFLICT,
            error: ErrorBody {
                code: Some("lattice_mode_unsupported"),
                kind: "dependency",
                message: "reindexPost kicks embedded sqlite only; lattice.mode=http is out of scope".into(),
                exit: 3,
                diagnostics: vec![Diagnostic {
                    fact: "lattice.mode".into(),
                    value: json!("http"),
                    evidence: Some("vaultInfoGet".into()),
                }],
                recovery: vec![Recovery {
                    action: "reindex",
                    detail: "set lattice.mode=embedded; reindexPost never writes lattice.db".into(),
                    disk_inventory_allowed: None,
                }],
                miss_id: None,
            },
            meta: Meta::default(),
        };
    }
    let rel = match notes::resolve(vault, &req.path) {
        Ok((rel, _)) => rel,
        Err(e) => return engine_api(e),
    };
    match backend.reindex(&rel).await {
        Ok(r) => ApiV1::Ok { data: r, meta: Meta::default() },
        Err(e) => engine_api(e),
    }
}

fn engine_api<T>(e: LapisError) -> ApiV1<T> {
    let (status, error) = map_engine(&e);
    ApiV1::Err { status, error, meta: Meta::default() }
}

fn mode_unsupported(message: &str, miss_id: Option<&'static str>) -> ErrorBody {
    let mut recovery = vec![Recovery {
        action: "set_lattice_http",
        detail: "pass lattice_url or set config lattice.mode=http".into(),
        disk_inventory_allowed: None,
    }];
    if miss_id == Some(FORENSIC_TREE) {
        recovery.push(Recovery {
            action: "use_neighbors",
            detail: "neighborsGet on a known hub path".into(),
            disk_inventory_allowed: None,
        });
    }
    ErrorBody {
        code: Some("lattice_mode_unsupported"),
        kind: "dependency",
        message: message.into(),
        exit: 3,
        diagnostics: vec![Diagnostic {
            fact: "lattice.mode".into(),
            value: json!("embedded"),
            evidence: Some("vaultInfoGet".into()),
        }],
        recovery,
        miss_id,
    }
}

fn clean_note_path(path: &str) -> crate::error::Result<String> {
    let rel = notes::clean_rel(&percent_decode(path))?;
    Ok(if Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel })
}

fn parse_note_query(raw: Option<&str>) -> std::result::Result<NoteQuery, Box<Response>> {
    let mut q = NoteQuery::default();
    for (k, v) in form_pairs(raw) {
        match k.as_str() {
            "heading" => q.heading = Some(v),
            "chunk" => {
                q.chunk =
                    Some(v.parse().map_err(|_| Box::new(invalid_request("chunk must be an integer ≥ 0")))?);
            }
            "max_chars" => {
                let n: usize =
                    v.parse().map_err(|_| Box::new(invalid_request("max_chars must be an integer ≥ 1")))?;
                if n < 1 {
                    return Err(Box::new(invalid_request("max_chars must be an integer ≥ 1")));
                }
                q.max_chars = Some(n);
            }
            "meta_only" => q.meta_only = v == "true" || v == "1",
            _ => {}
        }
    }
    Ok(q)
}

fn parse_neighbors_query(raw: Option<&str>) -> std::result::Result<NeighborsQuery, Box<Response>> {
    let mut direction = "both".to_string();
    let mut dangling = false;
    let mut hop = 1u32;
    for (k, v) in form_pairs(raw) {
        match k.as_str() {
            "direction" => direction = v,
            "dangling" => dangling = v == "true" || v == "1",
            "hop" => {
                hop = v.parse().map_err(|_| Box::new(invalid_request("hop must be 1 or 2")))?;
            }
            _ => {}
        }
    }
    if !matches!(direction.as_str(), "out" | "in" | "both") {
        return Err(Box::new(invalid_request("direction must be out|in|both")));
    }
    if hop != 1 && hop != 2 {
        return Err(Box::new(invalid_request("hop must be 1 or 2")));
    }
    Ok(NeighborsQuery { direction, dangling, hop })
}

fn validate_tree(req: &TreeRetrieveRequest) -> Option<Response> {
    let path = req.path.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let query = req.query.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if path.is_none() && query.is_none() {
        return Some(invalid_request("tree-retrieve needs path or query"));
    }
    if !(1..=3).contains(&req.depth) {
        return Some(invalid_request("depth must be 1..3"));
    }
    if !(1..=200).contains(&req.max_nodes) {
        return Some(invalid_request("max_nodes must be 1..200"));
    }
    None
}

fn form_pairs(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    raw.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((percent_decode(&k.replace('+', " ")), percent_decode(&v.replace('+', " "))))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Filename-like query (SPEC-api §2): dotted basename, glob, or vault-relative path.
pub fn is_filename_like(query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return false;
    }
    if q.contains('*') || q.contains('?') {
        return true;
    }
    if q.contains('/') {
        return true;
    }
    match q.rsplit_once('.') {
        Some((name, ext)) => !name.is_empty() && !ext.is_empty() && !ext.contains(char::is_whitespace),
        None => false,
    }
}

fn path_match_for_query(query: &str) -> lapis_lattice::PathMatch {
    let q = query.trim();
    if q.contains('*') || q.contains('?') {
        lapis_lattice::PathMatch::Glob
    } else if q.contains('/') {
        lapis_lattice::PathMatch::Substring
    } else {
        lapis_lattice::PathMatch::Basename
    }
}

fn fuse_paths(mut content: SearchResult, paths: Vec<PathHit>, cap: u32) -> SearchResult {
    let mut seen: HashSet<String> = content.hits.iter().map(|h| h.path.clone()).collect();
    let mut head: Vec<Hit> = Vec::new();
    for p in paths {
        if seen.insert(p.path.clone()) {
            head.push(path_to_hit(p));
        }
    }
    head.extend(content.hits);
    if cap > 0 {
        head.truncate(cap as usize);
    }
    for (i, h) in head.iter_mut().enumerate() {
        h.rank = Some((i as u32) + 1);
    }
    if !content.modalities.iter().any(|m| m == "path") {
        content.modalities.push("path".into());
    }
    content.hits = head;
    content.count = content.hits.len();
    content
}

fn path_to_hit(p: PathHit) -> Hit {
    Hit {
        title: notes::stem_of(&p.path),
        kind: p.kind,
        heading: None,
        snippet: None,
        score: None,
        rank: None,
        domain: None,
        doc_type: None,
        tags: vec![],
        chunk_id: None,
        chunk_index: None,
        path: p.path,
    }
}

fn page_search(mut data: SearchResult, limit: u32, offset: u32) -> SearchResult {
    let skipped: Vec<Hit> = data.hits.into_iter().skip(offset as usize).collect();
    data.hits = if limit == 0 { skipped } else { skipped.into_iter().take(limit as usize).collect() };
    data.count = data.hits.len();
    data
}

fn search_meta(data: &SearchResult, limit: u32, offset: u32) -> Meta {
    let truncated = limit > 0 && data.count as u32 >= limit;
    Meta {
        truncated,
        next: if truncated { Some(offset + limit) } else { None },
        latency: Some(data.latency),
        count: Some(data.count),
        limit: Some(limit),
        offset: Some(offset),
        ..Meta::default()
    }
}

fn empty_search(req: &SearchRequest) -> SearchResult {
    SearchResult {
        query: req.query.trim().to_string(),
        mode: req.mode,
        modalities: vec![],
        latency_ms: None,
        latency: Default::default(),
        count: 0,
        hits: vec![],
    }
}

fn validate_search(req: &SearchRequest) -> Option<Response> {
    if req.query.trim().is_empty() {
        return Some(invalid_request("query is required"));
    }
    if !(1..=50).contains(&req.limit) {
        return Some(invalid_request("limit must be 1..50"));
    }
    let requested = req.offset.saturating_add(req.limit);
    if requested > 50 {
        return Some(invalid_request(&format!("offset + limit must be ≤ 50 (got {requested})")));
    }
    None
}

fn validate_paths(req: &PathsSearchRequest) -> Option<Response> {
    if req.pattern.trim().is_empty() {
        return Some(invalid_request("pattern is required"));
    }
    if !(1..=1000).contains(&req.limit) {
        return Some(invalid_request("limit must be 1..1000"));
    }
    None
}

fn decode_body<T: for<'de> Deserialize<'de>>(body: &Bytes) -> std::result::Result<T, Box<Response>> {
    if body.is_empty() {
        return Err(Box::new(invalid_request("JSON body is required")));
    }
    serde_json::from_slice(body).map_err(|e| Box::new(invalid_request(&format!("invalid JSON: {e}"))))
}

fn invalid_request(message: &str) -> Response {
    err_json(
        StatusCode::BAD_REQUEST,
        ErrorBody {
            code: Some("invalid_request"),
            kind: "usage",
            message: message.into(),
            exit: 1,
            diagnostics: vec![],
            recovery: vec![Recovery {
                action: "paths_search",
                detail: "correct the request body and retry".into(),
                disk_inventory_allowed: None,
            }],
            miss_id: None,
        },
        Meta::default().with_api_version(),
    )
}

fn index_gap_error(e: &LapisError) -> ErrorBody {
    ErrorBody {
        code: Some("index_gap"),
        kind: "dependency",
        message: format!("index path table cannot answer: {e}"),
        exit: 2,
        diagnostics: vec![Diagnostic {
            fact: "cause".into(),
            value: json!(e.to_string()),
            evidence: Some("pathsSearchPost".into()),
        }],
        recovery: vec![
            Recovery {
                action: "reindex",
                detail: "POST /v1/reindex for a known path, then retry paths/search".into(),
                disk_inventory_allowed: None,
            },
            Recovery {
                action: "start_lattice",
                detail: "or switch lattice.mode=http if the HTTP indexer is the SoT".into(),
                disk_inventory_allowed: None,
            },
        ],
        miss_id: None,
    }
}

fn map_engine(e: &LapisError) -> (StatusCode, ErrorBody) {
    match e {
        LapisError::Usage(m) => (
            StatusCode::BAD_REQUEST,
            ErrorBody {
                code: Some("invalid_request"),
                kind: "usage",
                message: m.clone(),
                exit: 1,
                diagnostics: vec![],
                recovery: vec![Recovery {
                    action: "paths_search",
                    detail: "correct the request and retry".into(),
                    disk_inventory_allowed: None,
                }],
                miss_id: None,
            },
        ),
        LapisError::LatticeDown(m) => (
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorBody {
                code: Some("lattice_unreachable"),
                kind: "dependency",
                message: m.clone(),
                exit: 2,
                diagnostics: vec![],
                recovery: vec![Recovery {
                    action: "start_lattice",
                    detail: "start the lattice indexer or repair the embedded sqlite".into(),
                    disk_inventory_allowed: None,
                }],
                miss_id: None,
            },
        ),
        LapisError::Path(m) => (
            StatusCode::NOT_FOUND,
            ErrorBody {
                code: Some("not_found"),
                kind: "not_found",
                message: m.clone(),
                exit: 3,
                diagnostics: vec![],
                recovery: vec![Recovery {
                    action: "paths_search",
                    detail: "POST /v1/paths/search with the basename".into(),
                    disk_inventory_allowed: None,
                }],
                miss_id: None,
            },
        ),
        LapisError::Internal(m) => (
            StatusCode::BAD_REQUEST,
            ErrorBody {
                code: Some("invalid_request"),
                kind: "internal",
                message: m.clone(),
                exit: 1,
                diagnostics: vec![],
                recovery: vec![Recovery {
                    action: "reindex",
                    detail: "retry after reindex; report if it persists".into(),
                    disk_inventory_allowed: None,
                }],
                miss_id: None,
            },
        ),
    }
}

fn from_engine(e: &LapisError, meta: Meta) -> Response {
    let (status, error) = map_engine(e);
    err_json(status, error, meta.with_api_version())
}

fn ok_json<T: Serialize>(data: T, meta: Meta) -> Response {
    (StatusCode::OK, Json(envelope::ok(data, meta))).into_response()
}

fn err_json(status: StatusCode, error: ErrorBody, meta: Meta) -> Response {
    let env: Envelope<Value> = Envelope { ok: false, data: None, error: Some(error), meta };
    (status, Json(env)).into_response()
}

fn ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn temp_vault() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("lapis-a1-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(d.join("agents")).unwrap();
        std::fs::write(
            d.join("Welcome.md"),
            "# Welcome\n\nSee [[AGENTS]] and [[Missing]].\nHello lattice.\n",
        )
        .unwrap();
        std::fs::write(d.join("AGENTS.md"), "# AGENTS\n\nRoot agents file.\n").unwrap();
        std::fs::write(d.join("agents/AGENTS.md"), "# nested AGENTS\n").unwrap();
        d
    }

    fn backend_for(root: &std::path::Path) -> Backend {
        let mut e = lapis_lattice::Engine::open(root).unwrap();
        e.reindex().unwrap();
        Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(e)))
    }

    fn search_req(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            limit: 10,
            offset: 0,
            domain: None,
            mode: Mode::Hybrid,
            per_doc: true,
        }
    }

    const FORENSIC_AGENTS_MD: &str = "lapis.forensic.search.agents_md_silent_zero";

    fn search_incomplete(query: &str) -> ErrorBody {
        let miss = if query.trim() == "AGENTS.md" || query.trim().ends_with(".AGENTS.md") {
            Some(FORENSIC_AGENTS_MD)
        } else {
            None
        };
        ErrorBody {
            code: Some("search_incomplete"),
            kind: "dependency",
            message: "filename-like query did not run the path channel".into(),
            exit: 2,
            diagnostics: vec![
                Diagnostic { fact: "query".into(), value: json!(query), evidence: None },
                Diagnostic { fact: "path_channel".into(), value: json!(false), evidence: None },
            ],
            recovery: vec![Recovery {
                action: "paths_search",
                detail: format!("POST /v1/paths/search {{\"pattern\": {query:?}, \"match\": \"basename\"}}"),
                disk_inventory_allowed: None,
            }],
            miss_id: miss,
        }
    }

    #[test]
    fn search_incomplete_miss_id_for_agents_md() {
        let e = search_incomplete("AGENTS.md");
        assert_eq!(e.code, Some("search_incomplete"));
        assert_eq!(e.kind, "dependency");
        assert_eq!(e.miss_id, Some(FORENSIC_AGENTS_MD));
        assert_eq!(e.recovery[0].action, "paths_search");
    }

    #[test]
    fn listen_addr_loopback_only() {
        assert!(listen_addr("127.0.0.1", 18765).is_ok());
        assert!(listen_addr("localhost", 18765).is_ok());
        let loopback = bind_policy("127.0.0.1", 18765, None).unwrap();
        assert!(loopback.1.is_none());
        assert!(bind_policy("0.0.0.0", 18765, None).is_err());
        assert!(bind_policy("8.8.8.8", 18765, Some("tok")).is_err());
        assert!(bind_policy("100.89.131.70", 18765, None).is_err());
        let ts = bind_policy("100.89.131.70", 18765, Some("secret")).unwrap();
        assert_eq!(ts.1.as_deref(), Some("secret"));
    }

    #[test]
    fn filename_like_rules() {
        assert!(is_filename_like("AGENTS.md"));
        assert!(is_filename_like("SQUAD.AGENTS.md"));
        assert!(is_filename_like("*.md"));
        assert!(is_filename_like("**/AGENTS.md"));
        assert!(is_filename_like("agents/AGENTS.md"));
        assert!(!is_filename_like("welcome overlay"));
        assert!(!is_filename_like("AGENTS"));
        assert!(!is_filename_like(""));
    }

    #[tokio::test]
    async fn search_agents_md_never_silent_zero() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match search_v1(&backend, search_req("AGENTS.md")).await {
            SearchV1::Ok { data, meta } => {
                assert!(
                    data.count > 0,
                    "silent zero: ok:true count:0 for AGENTS.md (hits={:?})",
                    data.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
                );
                assert_eq!(meta.search_capable, Some(true));
                assert!(
                    data.hits.iter().any(|h| h.path.ends_with("AGENTS.md")),
                    "expected an AGENTS.md path hit, got {:?}",
                    data.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
                );
            }
            SearchV1::Err { error, .. } => {
                assert_eq!(error.code, Some("search_incomplete"));
                assert_eq!(error.miss_id, Some(FORENSIC_AGENTS_MD));
                panic!("preferred path hits; got search_incomplete: {}", error.message);
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn capable_true_empty_is_ok() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match search_v1(&backend, search_req("zz-no-such-token-xyz")).await {
            SearchV1::Ok { data, meta } => {
                assert_eq!(data.count, 0);
                assert!(data.hits.is_empty());
                assert_eq!(meta.search_capable, Some(true));
            }
            SearchV1::Err { error, .. } => panic!("true empty must be ok:true, got {}", error.message),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn paths_search_basename_agents_md() {
        let d = temp_vault();
        let backend = backend_for(&d);
        let req = PathsSearchRequest {
            pattern: "AGENTS.md".into(),
            match_kind: PathMatchWire::Basename,
            limit: 50,
        };
        let PathsV1::Ok { data, meta } = paths_search_v1(&backend, &req).await else {
            panic!("path table");
        };
        assert!(data.count >= 2, "expected root + nested AGENTS.md, got {:?}", data.hits);
        assert!(data.hits.iter().any(|h| h.path == "AGENTS.md"));
        assert!(data.hits.iter().any(|h| h.path == "agents/AGENTS.md"));
        assert_eq!(meta.search_capable, Some(true));
        assert_ne!(meta.incomplete, Some(true), "incomplete is not a substitute for hits");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn health_envelope_has_honest_version() {
        let app = router(dummy_state());
        let res =
            app.oneshot(Request::builder().uri("/v1/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get(axum::http::header::CONTENT_TYPE).unwrap(), "application/json");
        let body = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert!(v["error"].is_null());
        assert_eq!(v["data"]["api_version"], API_VERSION);
        assert_eq!(v["data"]["lapis_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(v["meta"]["api_version"], API_VERSION);
        assert!(v.get("type").is_none() && v.get("title").is_none());
    }

    #[tokio::test]
    async fn search_bad_json_is_envelope_not_rfc7807() {
        let app = router(dummy_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let ct = res.headers().get(axum::http::header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert_eq!(ct, "application/json");
        assert!(!ct.contains("problem"));
        let body = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["data"].is_null());
        assert_eq!(v["error"]["code"], "invalid_request");
        assert!(v.get("status").is_none());
    }

    #[tokio::test]
    async fn note_get_reads_and_not_found() {
        let d = temp_vault();
        match note_get_v1(&d, "Welcome.md", &NoteQuery::default()) {
            ApiV1::Ok { data, .. } => {
                assert_eq!(data.path, "Welcome.md");
                assert!(data.body.contains("Welcome"));
            }
            ApiV1::Err { error, .. } => panic!("read Welcome.md: {}", error.message),
        }
        match note_get_v1(&d, "agents%2FAGENTS.md", &NoteQuery::default()) {
            ApiV1::Ok { data, .. } => assert_eq!(data.path, "agents/AGENTS.md"),
            ApiV1::Err { error, .. } => panic!("encoded path: {}", error.message),
        }
        match note_get_v1(&d, "nope.md", &NoteQuery::default()) {
            ApiV1::Err { status, error, .. } => {
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert_eq!(error.code, Some("not_found"));
                assert_eq!(error.kind, "not_found");
            }
            ApiV1::Ok { .. } => panic!("missing note must be not_found"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn neighbors_dangling_and_no_silent_hop2() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match neighbors_v1(
            &backend,
            "Welcome.md",
            &NeighborsQuery { direction: "both".into(), dangling: true, hop: 1 },
        )
        .await
        {
            ApiV1::Ok { data, .. } => {
                assert_eq!(data.hop, 1);
                assert_eq!(data.direction, "both");
                assert!(
                    data.neighbors.iter().any(|n| n.path.as_deref() == Some("AGENTS.md") && n.resolved),
                    "{:?}",
                    data.neighbors.iter().map(|n| &n.path).collect::<Vec<_>>()
                );
                assert!(
                    data.neighbors
                        .iter()
                        .any(|n| n.path.is_none() && n.dst_raw.as_deref() == Some("Missing")),
                    "dangling must have path=null"
                );
            }
            ApiV1::Err { error, .. } => panic!("hop-1: {}", error.message),
        }
        match neighbors_v1(
            &backend,
            "Welcome.md",
            &NeighborsQuery { direction: "both".into(), dangling: true, hop: 2 },
        )
        .await
        {
            ApiV1::Err { status, error, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(error.code, Some("lattice_mode_unsupported"));
                assert_ne!(error.miss_id, Some(FORENSIC_TREE));
            }
            ApiV1::Ok { data, .. } => {
                panic!("silent hop-2 on embedded ({} rows)", data.neighbors.len())
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn tree_retrieve_embedded_is_forensic_miss() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match tree_retrieve_v1(
            &backend,
            TreeRetrieveRequest { path: Some("Welcome.md".into()), query: None, depth: 2, max_nodes: 60 },
        )
        .await
        {
            ApiV1::Err { status, error, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(error.code, Some("lattice_mode_unsupported"));
                assert_eq!(error.kind, "dependency");
                assert_eq!(error.miss_id, Some(FORENSIC_TREE));
                assert!(error.recovery.iter().any(|r| r.action == "set_lattice_http"));
                assert!(error.recovery.iter().any(|r| r.action == "use_neighbors"));
            }
            ApiV1::Ok { .. } => panic!("embedded tree-retrieve must be lattice_mode_unsupported"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn analytics_allowlist_rejects_sql() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match analytics_v1(&backend, "inventory").await {
            ApiV1::Ok { data, .. } => assert_eq!(data.query, "inventory"),
            ApiV1::Err { error, .. } => panic!("inventory: {}", error.message),
        }
        for bad in ["select 1", "DROP TABLE documents", "nope"] {
            match analytics_v1(&backend, bad).await {
                ApiV1::Err { status, error, .. } => {
                    assert_eq!(status, StatusCode::BAD_REQUEST);
                    assert_eq!(error.code, Some("invalid_request"));
                }
                ApiV1::Ok { .. } => panic!("{bad} must be invalid_request"),
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn reindex_embedded_only_never_http() {
        let d = temp_vault();
        let backend = backend_for(&d);
        match reindex_v1(&backend, &d, ReindexRequest { path: "Welcome.md".into() }).await {
            ApiV1::Ok { data, .. } => assert_eq!(data.path, "Welcome.md"),
            ApiV1::Err { error, .. } => panic!("embedded reindex: {}", error.message),
        }
        match reindex_v1(&backend, &d, ReindexRequest { path: "missing.md".into() }).await {
            ApiV1::Err { error, .. } => assert_eq!(error.code, Some("not_found")),
            ApiV1::Ok { .. } => panic!("missing path must be not_found"),
        }
        let http =
            crate::lattice::Client::new("http://127.0.0.1:9", std::time::Duration::from_millis(50)).unwrap();
        match reindex_v1(&Backend::Http(http), &d, ReindexRequest { path: "Welcome.md".into() }).await {
            ApiV1::Err { status, error, .. } => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(error.code, Some("lattice_mode_unsupported"));
            }
            ApiV1::Ok { .. } => panic!("http reindex must not write lattice.db"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn bearer_required_when_token_configured() {
        let mut st = dummy_state();
        st.token = Some("secret".into());
        let app = router(st);
        let res =
            app.oneshot(Request::builder().uri("/v1/health").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "unauthorized");
        assert_eq!(v["error"]["kind"], "auth");
    }

    fn dummy_state() -> ApiState {
        let d = temp_vault();
        let backend = backend_for(&d);
        // Leak the temp dir for the dummy health-only state; tests that need
        // files use their own vault. Health does not read the index.
        let vault = crate::vault::Vault { root: d, source: "flag" };
        let ctx = Ctx {
            json: true,
            vault,
            cfg: crate::config::Config::default(),
            lattice_url: crate::config::DEFAULT_LATTICE_URL.into(),
            force_http: false,
        };
        ApiState { ctx: Arc::new(ctx), backend, token: None }
    }
}
