#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub is_active: bool,
    pub must_change_password: bool,
    pub totp_secret: Option<String>,
    pub totp_enabled: bool,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}
