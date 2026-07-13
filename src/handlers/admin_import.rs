use askama::Template;
use axum::Extension;
use axum::extract::{Multipart, State};
use axum::response::{Html, IntoResponse};

use crate::AppState;
use crate::bgcatalog_import::{self, ImportSummary};
use crate::security::CurrentUser;

#[derive(Template)]
#[template(path = "admin_import_bgcatalog.html")]
struct ImportFormTemplate {
    title: String,
    username: String,
    error: Option<String>,
}

pub async fn import_form(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        ImportFormTemplate {
            title: "Import from BG Catalog".to_string(),
            username: current.username,
            error: None,
        }
        .render()
        .unwrap(),
    )
}

#[derive(Template)]
#[template(path = "admin_import_result.html")]
struct ImportResultTemplate {
    title: String,
    username: String,
    summary: ImportSummary,
}

pub async fn run_import(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut zip_bytes: Option<Vec<u8>> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("export_zip") {
            if let Ok(bytes) = field.bytes().await {
                zip_bytes = Some(bytes.to_vec());
            }
        }
    }

    let Some(zip_bytes) = zip_bytes else {
        return Html(
            ImportFormTemplate {
                title: "Import from BG Catalog".to_string(),
                username: current.username,
                error: Some("Please choose a .zip file to upload.".to_string()),
            }
            .render()
            .unwrap(),
        )
        .into_response();
    };

    match bgcatalog_import::import_from_zip(&state, zip_bytes, current.id).await {
        Ok(summary) => Html(
            ImportResultTemplate {
                title: "Import complete".to_string(),
                username: current.username,
                summary,
            }
            .render()
            .unwrap(),
        )
        .into_response(),
        Err(e) => Html(
            ImportFormTemplate {
                title: "Import from BG Catalog".to_string(),
                username: current.username,
                error: Some(e),
            }
            .render()
            .unwrap(),
        )
        .into_response(),
    }
}
