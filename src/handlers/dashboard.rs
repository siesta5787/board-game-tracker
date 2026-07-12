use askama::Template;
use axum::Extension;
use axum::response::{Html, IntoResponse};

use crate::security::CurrentUser;

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    title: String,
    username: String,
    is_admin: bool,
}

pub async fn home(Extension(CurrentUser(user)): Extension<CurrentUser>) -> impl IntoResponse {
    Html(
        DashboardTemplate {
            title: "Home".to_string(),
            username: user.username,
            is_admin: user.is_admin,
        }
        .render()
        .unwrap(),
    )
}
