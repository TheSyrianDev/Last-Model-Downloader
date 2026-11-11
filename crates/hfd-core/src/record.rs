//! Log of completed downloads (`downloads.json`), for tools that keep a
//! model list. Enabled by `DownloadOptions::download_log`.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{DownloadedFile, RepoRef, parse_quant};
use serde_json::{Value, json};

/// Appends (or replaces, keyed by absolute path) one entry. Errors are
/// returned so callers can warn; they never fail the download.
pub fn record_download(
    log: &Path,
    repo: &RepoRef,
    revision: &str,
    file: &DownloadedFile,
) -> crate::Result<()> {
    append(log, repo, revision, file)
        .map_err(|error| crate::Error::Invalid(format!("download log {}: {error}", log.display())))
}

fn append(
    log: &Path,
    repo: &RepoRef,
    revision: &str,
    file: &DownloadedFile,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = fs::canonicalize(&file.local_path).unwrap_or_else(|_| file.local_path.clone());
    let path = path.to_string_lossy().to_string();
    let mut doc: Value = fs::read(log)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({ "schema": 1, "downloads": [] }));
    if !doc["downloads"].is_array() {
        doc["downloads"] = json!([]);
    }
    let list = doc["downloads"].as_array_mut().unwrap();
    list.retain(|entry| entry["path"].as_str() != Some(path.as_str()));
    list.push(json!({
        "repo": repo.repo_id,
        "file": file.path,
        "quant": parse_quant(&file.path).map(|quant| quant.name),
        "size_bytes": file.size,
        "sha256": file.sha256,
        "path": path,
        "revision": revision,
        "downloaded_at": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    }));
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = log.with_extension(format!("tmp.{}", std::process::id()));
    fs::write(&tmp, serde_json::to_vec_pretty(&doc)?)?;
    fs::rename(&tmp, log)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RepoType;

    #[test]
    fn appends_and_replaces_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("downloads.json");
        let model = dir.path().join("m-Q4_K_M.gguf");
        fs::write(&model, b"x").unwrap();
        let file = DownloadedFile {
            path: "m-Q4_K_M.gguf".into(),
            local_path: model,
            size: 1,
            skipped: false,
            resumed_bytes: 0,
            engine: "http",
            sha256: Some("ab".into()),
        };
        let repo = RepoRef::new(RepoType::Model, "o/r");
        record_download(&log, &repo, "main", &file).unwrap();
        record_download(&log, &repo, "main", &file).unwrap();
        let doc: Value = serde_json::from_slice(&fs::read(&log).unwrap()).unwrap();
        let list = doc["downloads"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["quant"], "Q4_K_M");
        assert_eq!(list[0]["sha256"], "ab");
    }
}
