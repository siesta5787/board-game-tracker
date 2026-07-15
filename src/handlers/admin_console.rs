//! Landing page for all admin-only tools, linked from Settings — a single
//! entry point instead of five separate links, so the admin surface reads
//! as one coherent area rather than a scattered list.

use askama::Template;
use axum::Extension;
use axum::response::{Html, IntoResponse};

use crate::security::CurrentUser;

#[derive(Template)]
#[template(path = "admin_console.html")]
struct AdminConsoleTemplate {
    title: String,
    username: String,
}

pub async fn show_console(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    Html(
        AdminConsoleTemplate {
            title: "Admin console".to_string(),
            username: current.username,
        }
        .render()
        .unwrap(),
    )
}
