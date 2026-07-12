use axum::Extension;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;

use crate::AppState;
use crate::data_export;
use crate::security::CurrentUser;

pub async fn export_data(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    match data_export::build_export(&state, current.id).await {
        Ok(zip_bytes) => {
            let filename = format!("{}-export.zip", current.username);
            let headers = [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ];
            (headers, zip_bytes).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Export failed: {e}"),
        )
            .into_response(),
    }
}
