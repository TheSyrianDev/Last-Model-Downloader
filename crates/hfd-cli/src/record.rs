//! Optional log of completed downloads, for tools that keep a model list.
//! Set `HFD_DOWNLOAD_LOG=/path/downloads.json` to enable it.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use hfd_core::{DownloadedFile, RepoRef, parse_quant};
use serde_json::{Value, json};

pub fn record_download(repo: &RepoRef, revision: &str, file: &DownloadedFile) {
    let Some(log) = std::env::var_os("HFD_DOWNLOAD_LOG") else {
        return;
    };
    if let Err(error) = append(Path::new(&log), repo, revision, file) {
        eprintln!(
            "warning: could not update {}: {error}",
            Path::new(&log).display()
        );
    }
}

fn append(log: &Path, repo: &RepoRef, revision: &str, file: &DownloadedFile) -> anyhow::Result<()> {
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
