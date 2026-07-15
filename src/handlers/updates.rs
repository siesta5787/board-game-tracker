//! Combined admin view for update-related actions: the app's own version
//! and self-update controls alongside the Pi's OS package updates and
//! Tailscale. These were two separate pages (Software update / System
//! updates) — merged into one per admin feedback, since "is everything up
//! to date" is one job regardless of which layer it's checking. The two
//! underlying schedules (app auto-update vs. OS/Tailscale auto-update)
//! stay independently configurable — only the page is combined; each
//! POST route from handlers::system_update / handlers::system_maintenance
//! still exists and hands off to `render_page` below afterward.

use askama::Template;
use axum::Extension;
use axum::response::{Html, IntoResponse};

use crate::handlers::{system_maintenance, system_update};
use crate::models::User;
use crate::security::CurrentUser;

#[derive(Template)]
#[template(path = "admin_updates.html")]
struct UpdatesTemplate {
    title: String,
    username: String,
    message: Option<String>,
    app: system_update::AppUpdateData,
    os: system_maintenance::OsUpdateData,
}

pub(crate) async fn render_page(current: &User, message: Option<String>) -> Html<String> {
    Html(
        UpdatesTemplate {
            title: "Software updates".to_string(),
            username: current.username.clone(),
            message,
            app: system_update::gather().await,
            os: system_maintenance::gather().await,
        }
        .render()
        .unwrap(),
    )
}

pub async fn show_updates_page(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_page(&current, None).await
}
