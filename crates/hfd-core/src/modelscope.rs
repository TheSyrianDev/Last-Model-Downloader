//! ModelScope source: list a repository, pick files, and transfer them over
//! plain HTTP with sha256 verification. ModelScope's hub layout is close to the
//! Hugging Face one but its API is not: listings come from
//! `/api/v1/{models|datasets}/{repo}/repo/files`, and each file is fetched from
//! `/api/v1/{models|datasets}/{repo}/repo?FilePath=`. Revisions are called
//! `Revision` there, and the default branch is `master`.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use globset::Glob;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::uri::RepoType;

pub const DEFAULT_API: &str = "https://modelscope.cn";

/// Default branch name on ModelScope. `main` exists too, but `master` is what
/// the site and the SDK use when a revision is not given.
pub const DEFAULT_REVISION: &str = "master";

/// One file in a ModelScope repository. `sha256` is absent when the listing
/// does not publish a digest; such a file cannot be verified, only sized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: Option<String>,
}

fn segment(repo_type: RepoType) -> Result<&'static str> {
    match repo_type {
        RepoType::Model => Ok("models"),
        RepoType::Dataset => Ok("datasets"),
        RepoType::Space => Err(Error::Invalid(
            "ModelScope has no spaces; use --type model or --type dataset".to_string(),
        )),
    }
}

fn revision_or_default(revision: Option<&str>) -> &str {
    revision.unwrap_or(DEFAULT_REVISION)
}

/// Percent-encode everything except the unreserved set and `/`, which every
/// caller here wants kept literally (repo ids, and file paths inside them).
fn encode_keep_slash(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `GET /api/v1/models/{repo}/repo/files?Revision=master&Recursive=True`.
pub fn files_url(
    api: &str,
    repo_type: RepoType,
    repo_id: &str,
    revision: Option<&str>,
) -> Result<String> {
    if repo_id.is_empty() {
        return Err(Error::Invalid("repository is empty".to_string()));
    }
    Ok(format!(
        "{}/api/v1/{}/{}/repo/files?Revision={}&Recursive=True",
        api.trim_end_matches('/'),
        segment(repo_type)?,
        encode_keep_slash(repo_id.trim_matches('/')),
        encode_keep_slash(revision_or_default(revision)),
    ))
}

/// `GET /api/v1/models/{repo}/repo?Revision=master&FilePath={path}`.
pub fn download_url(
    api: &str,
    repo_type: RepoType,
    repo_id: &str,
    revision: Option<&str>,
    path: &str,
) -> Result<String> {
    crate::uri::validate_repo_path(path)?;
    Ok(format!(
        "{}/api/v1/{}/{}/repo?Revision={}&FilePath={}",
        api.trim_end_matches('/'),
        segment(repo_type)?,
        encode_keep_slash(repo_id.trim_matches('/')),
        encode_keep_slash(revision_or_default(revision)),
        encode_keep_slash(path),
    ))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse a `repo/files` body. Directories are dropped, and a blob whose digest
/// is present but malformed is an error rather than a silently unverifiable file.
pub fn parse_files(json: &str) -> Result<Vec<FileEntry>> {
    let root: Value = serde_json::from_str(json)?;

    let code = root.get("Code").and_then(|c| {
        c.as_i64()
            .or_else(|| c.as_str().and_then(|s| s.parse::<i64>().ok()))
    });
    let ok = match code {
        Some(200) => true,
        Some(_) => false,
        None => root
            .get("Success")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    if !ok {
        let message = root
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(Error::Invalid(format!("ModelScope: {message}")));
    }

    let files = root
        .get("Data")
        .and_then(|d| d.get("Files"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Invalid("ModelScope: response has no file list".into()))?;

    let mut out = Vec::with_capacity(files.len());
    for entry in files {
        let kind = entry.get("Type").and_then(Value::as_str).unwrap_or("blob");
        if kind.eq_ignore_ascii_case("tree") || kind.eq_ignore_ascii_case("dir") {
            continue;
        }
        let path = entry
            .get("Path")
            .or_else(|| entry.get("Name"))
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("ModelScope: file without a path".into()))?
            .trim_start_matches('/')
            .to_string();
        crate::uri::validate_repo_path(&path)?;
        let size = entry.get("Size").and_then(Value::as_u64).unwrap_or(0);
        let sha256 = entry
            .get("Sha256")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase());
        if let Some(digest) = &sha256
            && !is_sha256(digest)
        {
            return Err(Error::Invalid(format!(
                "ModelScope: bad sha256 for {path}"
            )));
        }
        out.push(FileEntry { path, size, sha256 });
    }
    Ok(out)
}

fn matches_any(globs: &[Glob], path: &str) -> bool {
    globs.iter().any(|g| g.compile_matcher().is_match(path))
}

/// Keep files that match one of `wanted` (exact path, or a prefix ending in `/`),
/// pass the include globs when any were given, and never match an exclude glob.
/// Empty `wanted` keeps everything the globs allow.
pub fn select<'a>(
    files: &'a [FileEntry],
    wanted: &[String],
    include: &[Glob],
    exclude: &[Glob],
) -> Vec<&'a FileEntry> {
    files
        .iter()
        .filter(|f| {
            wanted.is_empty()
                || wanted
                    .iter()
                    .any(|w| f.path == *w || (w.ends_with('/') && f.path.starts_with(w.as_str())))
        })
        .filter(|f| include.is_empty() || matches_any(include, &f.path))
        .filter(|f| !matches_any(exclude, &f.path))
        .collect()
}

/// Token for private repositories. Public ones need none.
pub fn token() -> Option<String> {
    std::env::var("MODELSCOPE_API_TOKEN")
        .ok()
        .or_else(|| std::env::var("MODELSCOPE_TOKEN").ok())
        .filter(|t| !t.trim().is_empty())
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("model-downloader/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| Error::Invalid(format!("ModelScope client: {e}")))
}

fn with_auth(request: reqwest::RequestBuilder, api: &str) -> reqwest::RequestBuilder {
    // The token is only for the official host; a mirror or a mock server gets
    // no credentials.
    let official = reqwest::Url::parse(api)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with("modelscope.cn")))
        .unwrap_or(false);
    match (official, token()) {
        (true, Some(token)) => request.bearer_auth(token),
        _ => request,
    }
}

/// List the blobs in a repository.
pub async fn list(
    api: &str,
    repo_type: RepoType,
    repo_id: &str,
    revision: Option<&str>,
) -> Result<Vec<FileEntry>> {
    let url = files_url(api, repo_type, repo_id, revision)?;
    let response = with_auth(client()?.get(&url), api)
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("ModelScope unreachable: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| Error::Invalid(format!("ModelScope body: {e}")))?;
    if status.as_u16() == 404 {
        return Err(Error::Invalid(format!(
            "{repo_id} is not on ModelScope (HTTP 404)"
        )));
    }
    if !status.is_success() {
        return Err(Error::Invalid(format!(
            "ModelScope listing failed: HTTP {}",
            status.as_u16()
        )));
    }
    parse_files(&body)
}

/// Download one file into `dest`, resuming nothing (ModelScope serves whole
/// files) and verifying the published sha256 when there is one. A mismatch
/// deletes the partial file and fails.
pub async fn download(
    api: &str,
    repo_type: RepoType,
    repo_id: &str,
    revision: Option<&str>,
    file: &FileEntry,
    dest: &Path,
) -> Result<PathBuf> {
    let url = download_url(api, repo_type, repo_id, revision, &file.path)?;
    let local = dest.join(&file.path);
    if let Some(parent) = local.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let response = with_auth(client()?.get(&url), api)
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("ModelScope unreachable: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Error::Invalid(format!(
            "ModelScope download of {} failed: HTTP {}",
            file.path,
            status.as_u16()
        )));
    }

    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut out = tokio::fs::File::create(&local).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Invalid(format!("ModelScope transfer: {e}")))?;
        hasher.update(&chunk);
        written += chunk.len() as u64;
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);

    let actual = format!("{:x}", hasher.finalize());
    if let Some(expected) = &file.sha256
        && !actual.eq_ignore_ascii_case(expected)
    {
        let _ = tokio::fs::remove_file(&local).await;
        return Err(Error::Invalid(format!(
            "sha256 mismatch for {}: expected {expected}, got {actual}; file removed",
            file.path
        )));
    }
    if file.size > 0 && written != file.size {
        let _ = tokio::fs::remove_file(&local).await;
        return Err(Error::Invalid(format!(
            "size mismatch for {}: expected {} bytes, got {written}; file removed",
            file.path, file.size
        )));
    }
    Ok(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{
      "Code": 200,
      "Success": true,
      "Data": {
        "Files": [
          {"Type": "blob", "Path": "config.json", "Name": "config.json", "Size": 712,
           "Sha256": "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"},
          {"Type": "blob", "Path": "sub/model.gguf", "Name": "model.gguf", "Size": 3,
           "Sha256": ""},
          {"Type": "tree", "Path": "sub", "Name": "sub", "Size": 0}
        ]
      }
    }"#;

    #[test]
    fn lists_blobs_and_drops_directories() {
        let files = parse_files(LISTING).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "config.json");
        assert_eq!(
            files[0].sha256.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(files[1].sha256, None, "an empty digest is 'unknown', not zero");
    }

    #[test]
    fn reports_the_server_message_on_failure() {
        let body = r#"{"Code":10013201001,"Message":"too many requests","Success":false}"#;
        let err = parse_files(body).unwrap_err().to_string();
        assert!(err.contains("too many requests"), "got {err}");
    }

    #[test]
    fn rejects_bad_digest_and_unsafe_paths() {
        let bad = LISTING.replace("BA7816BF", "ZZ7816BF");
        assert!(parse_files(&bad).is_err());
        let evil = LISTING.replace("\"sub/model.gguf\"", "\"../model.gguf\"");
        assert!(parse_files(&evil).is_err());
        assert!(parse_files("{not json").is_err());
        assert!(parse_files(r#"{"Code":200,"Data":{}}"#).is_err());
    }

    #[test]
    fn builds_listing_urls() {
        assert_eq!(
            files_url(DEFAULT_API, RepoType::Model, "Qwen/Qwen3-0.6B", None).unwrap(),
            "https://modelscope.cn/api/v1/models/Qwen/Qwen3-0.6B/repo/files?Revision=master&Recursive=True"
        );
        assert_eq!(
            files_url("https://example.test/", RepoType::Dataset, "org/data", Some("v1"))
                .unwrap(),
            "https://example.test/api/v1/datasets/org/data/repo/files?Revision=v1&Recursive=True"
        );
        assert!(files_url(DEFAULT_API, RepoType::Space, "a/b", None).is_err());
    }

    #[test]
    fn builds_download_urls_and_encodes_the_path() {
        assert_eq!(
            download_url(DEFAULT_API, RepoType::Model, "a/b", Some("master"), "sub/my file.gguf")
                .unwrap(),
            "https://modelscope.cn/api/v1/models/a/b/repo?Revision=master&FilePath=sub/my%20file.gguf"
        );
        assert!(download_url(DEFAULT_API, RepoType::Model, "a/b", None, "../x").is_err());
    }

    #[test]
    fn selects_by_path_prefix_and_globs() {
        let files = parse_files(LISTING).unwrap();
        assert_eq!(select(&files, &[], &[], &[]).len(), 2);
        assert_eq!(select(&files, &["config.json".into()], &[], &[]).len(), 1);
        assert_eq!(select(&files, &["sub/".into()], &[], &[])[0].path, "sub/model.gguf");
        let gguf = [Glob::new("*.gguf").unwrap()];
        assert_eq!(select(&files, &[], &gguf, &[]).len(), 1);
        let skip = [Glob::new("config*").unwrap()];
        assert_eq!(select(&files, &[], &[], &skip).len(), 1);
    }
}
