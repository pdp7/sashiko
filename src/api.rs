use crate::db::Database;
use crate::settings::ServerSettings;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, get_service},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};
use tracing::info;

pub struct AppState {
    pub db: Arc<Database>,
}

#[derive(Deserialize)]
pub struct Pagination {
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

#[derive(Serialize)]
pub struct PatchsetsResponse {
    pub items: Vec<crate::db::PatchsetRow>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
}

#[derive(Deserialize)]
pub struct PatchQuery {
    pub id: String,
}

pub async fn run_server(
    settings: ServerSettings,
    db: Arc<Database>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(AppState { db });

    let app = Router::new()
        .route("/api/patchsets", get(list_patchsets))
        .route("/api/patch", get(get_patchset))
        .route("/api/stats", get(get_stats))
        .route("/", get_service(ServeFile::new("static/index.html")))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], settings.port));
    info!("Web API listening on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn list_patchsets(
    State(state): State<Arc<AppState>>,
    Query(pagination): Query<Pagination>,
) -> Result<Json<PatchsetsResponse>, StatusCode> {
    let page = pagination.page.unwrap_or(1).max(1);
    let per_page = pagination.per_page.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let items = state.db.get_patchsets(per_page, offset).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = state.db.count_patchsets().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(PatchsetsResponse {
        items,
        total,
        page,
        per_page,
    }))
}



async fn get_patchset(

    State(state): State<Arc<AppState>>,

    Query(query): Query<PatchQuery>,

) -> Result<Json<serde_json::Value>, StatusCode> {

    info!("Fetching details for message_id: {}", query.id);

    match state.db.get_patchset_details(&query.id).await {

        Ok(Some(details)) => Ok(Json(details)),

        Ok(None) => {

            info!("Message not found: {}", query.id);

            Err(StatusCode::NOT_FOUND)

        },

        Err(e) => {

            info!("Database error: {}", e);

            Err(StatusCode::INTERNAL_SERVER_ERROR)

        },

    }

}
async fn get_stats() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": "0.1.0"
    }))
}
