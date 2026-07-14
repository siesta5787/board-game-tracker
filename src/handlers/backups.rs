//! Admin-triggered backups of the whole app's data: a consistent snapshot of
//! the SQLite database plus every play photo, zipped up and stored under
//! `data/backups/` for the admin to download and store off the Pi's SD card.

use askama::Template;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse};
use std::io::Write;

use crate::AppState;
use crate::models::User;
use crate::security::CurrentUser;

const BACKUP_DIR: &str = "data/backups";

struct BackupRow {
    filename: String,
    size_display: String,
    created_display: String,
}

#[derive(Template)]
#[template(path = "admin_backups.html")]
struct BackupsTemplate {
    title: String,
    username: String,
    backups: Vec<BackupRow>,
    success: Option<String>,
    error: Option<String>,
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn is_safe_backup_filename(name: &str) -> bool {
    name.starts_with("backup-")
        && name.ends_with(".zip")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
}

async fn render_backups(
    current: &User,
    success: Option<String>,
    error: Option<String>,
) -> Html<String> {
    let mut backups: Vec<BackupRow> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(BACKUP_DIR).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let filename = entry.file_name().to_string_lossy().to_string();
            if !is_safe_backup_filename(&filename) {
                continue;
            }
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            let created_display = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                })
                .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_default();
            backups.push(BackupRow {
                filename,
                size_display: human_size(meta.len()),
                created_display,
            });
        }
    }
    backups.sort_by(|a, b| b.filename.cmp(&a.filename));

    Html(
        BackupsTemplate {
            title: "Backups".to_string(),
            username: current.username.clone(),
            backups,
            success,
            error,
        }
        .render()
        .unwrap(),
    )
}

pub async fn list_backups(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    render_backups(&current, None, None).await
}

fn build_backup_zip(db_snapshot_path: &str, zip_path: &str) -> std::io::Result<()> {
    let file = std::fs::File::create(zip_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    writer.start_file("boardgames.db", options)?;
    writer.write_all(&std::fs::read(db_snapshot_path)?)?;

    add_dir_to_zip(
        &mut writer,
        std::path::Path::new("data/photos"),
        "photos",
        options,
    )?;

    writer.finish()?;
    Ok(())
}

fn add_dir_to_zip(
    writer: &mut zip::ZipWriter<std::fs::File>,
    dir: &std::path::Path,
    zip_prefix: &str,
    options: zip::write::SimpleFileOptions,
) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let zip_path = format!("{zip_prefix}/{name}");
        if path.is_dir() {
            add_dir_to_zip(writer, &path, &zip_path, options)?;
        } else {
            writer.start_file(&zip_path, options)?;
            writer.write_all(&std::fs::read(&path)?)?;
        }
    }
    Ok(())
}

pub async fn create_backup(
    State(state): State<AppState>,
    Extension(CurrentUser(current)): Extension<CurrentUser>,
) -> impl IntoResponse {
    if tokio::fs::create_dir_all(BACKUP_DIR).await.is_err() {
        return render_backups(
            &current,
            None,
            Some("Couldn't create the backups folder.".to_string()),
        )
        .await
        .into_response();
    }

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let snapshot_path = format!("{BACKUP_DIR}/tmp-{timestamp}.db");
    let zip_path = format!("{BACKUP_DIR}/backup-{timestamp}.zip");

    // VACUUM INTO takes a consistent, complete snapshot of the live database
    // (including anything only committed to the WAL so far) without needing
    // to stop the app or risk a torn read from copying the file directly.
    let vacuum_result = sqlx::query("VACUUM INTO ?")
        .bind(&snapshot_path)
        .execute(&state.db)
        .await;

    if let Err(e) = vacuum_result {
        tracing::error!("backup snapshot failed: {e}");
        let _ = tokio::fs::remove_file(&snapshot_path).await;
        return render_backups(
            &current,
            None,
            Some("Something went wrong taking a database snapshot.".to_string()),
        )
        .await
        .into_response();
    }

    let snapshot_path_for_zip = snapshot_path.clone();
    let zip_path_for_zip = zip_path.clone();
    let zip_result = tokio::task::spawn_blocking(move || {
        build_backup_zip(&snapshot_path_for_zip, &zip_path_for_zip)
    })
    .await;

    let _ = tokio::fs::remove_file(&snapshot_path).await;

    match zip_result {
        Ok(Ok(())) => render_backups(&current, Some("Backup created.".to_string()), None)
            .await
            .into_response(),
        Ok(Err(e)) => {
            tracing::error!("failed to build backup zip: {e}");
            let _ = tokio::fs::remove_file(&zip_path).await;
            render_backups(
                &current,
                None,
                Some("Something went wrong building the backup file.".to_string()),
            )
            .await
            .into_response()
        }
        Err(e) => {
            tracing::error!("backup zip task panicked: {e}");
            render_backups(
                &current,
                None,
                Some("Something went wrong building the backup file.".to_string()),
            )
            .await
            .into_response()
        }
    }
}

pub async fn download_backup(Path(filename): Path<String>) -> impl IntoResponse {
    if !is_safe_backup_filename(&filename) {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    match tokio::fs::read(format!("{BACKUP_DIR}/{filename}")).await {
        Ok(bytes) => {
            let headers = [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ];
            (headers, bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

pub async fn delete_backup(
    Extension(CurrentUser(current)): Extension<CurrentUser>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    if is_safe_backup_filename(&filename) {
        let _ = tokio::fs::remove_file(format!("{BACKUP_DIR}/{filename}")).await;
    }
    render_backups(&current, Some("Backup deleted.".to_string()), None).await
}
