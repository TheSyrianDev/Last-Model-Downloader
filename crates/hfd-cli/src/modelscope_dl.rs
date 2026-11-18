//! `mdl download --source modelscope`: list a ModelScope repository, download
//! the selected files over HTTP, verify each sha256 the hub published, then
//! place them where the caller asked. No fallback to Hugging Face.
//!
//! The API base can be overridden with `MODELSCOPE_API` (a mirror or a test
//! server); a token for private repositories comes from
//! `MODELSCOPE_API_TOKEN`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use globset::Glob;
use hfd_core::modelscope;
use hfd_core::uri::RepoType;

pub struct Request<'a> {
    pub repo_id: &'a str,
    pub repo_type: RepoType,
    pub revision: Option<&'a str>,
    pub filenames: &'a [String],
    pub include: &'a [String],
    pub exclude: &'a [String],
    pub local_dir: Option<&'a Path>,
    pub flat: bool,
    pub cache_root: &'a Path,
    pub dry_run: bool,
}

pub async fn run(request: Request<'_>) -> Result<Vec<PathBuf>> {
    let api = std::env::var("MODELSCOPE_API")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| modelscope::DEFAULT_API.to_string());
    run_at(&api, request).await
}

fn globs(patterns: &[String]) -> Result<Vec<Glob>> {
    patterns
        .iter()
        .map(|p| Glob::new(p).with_context(|| format!("invalid glob '{p}'")))
        .collect()
}

/// Runner with the API base injected, so tests can point it at a mock server.
pub async fn run_at(api: &str, request: Request<'_>) -> Result<Vec<PathBuf>> {
    let include = globs(request.include)?;
    let exclude = globs(request.exclude)?;

    if request.local_dir.is_none() && !request.cache_root.exists() {
        std::fs::create_dir_all(request.cache_root).with_context(|| {
            format!("creating cache directory {}", request.cache_root.display())
        })?;
    }

    let files = modelscope::list(api, request.repo_type, request.repo_id, request.revision)
        .await
        .context("ModelScope listing")?;
    let selected = modelscope::select(&files, request.filenames, &include, &exclude);
    if selected.is_empty() {
        bail!(
            "no files in {} matched the request",
            request.repo_id
        );
    }

    if request.dry_run {
        for file in &selected {
            println!("{}\t{} bytes", file.path, file.size);
        }
        return Ok(Vec::new());
    }

    // Local directories get the bytes directly; the cache path stages first,
    // because the cache layout is written by `place_verified`.
    let staging = match request.local_dir {
        Some(dir) => dir.to_path_buf(),
        None => request
            .cache_root
            .join(".modelscope-staging")
            .join(request.repo_id.replace('/', "--")),
    };
    if request.local_dir.is_none() {
        std::fs::create_dir_all(&staging)?;
    }

    let mut fetched = Vec::new();
    for file in &selected {
        let path = modelscope::download(
            api,
            request.repo_type,
            request.repo_id,
            request.revision,
            file,
            &staging,
        )
        .await
        .with_context(|| format!("downloading {}", file.path))?;
        fetched.push(path);
    }

    let Some(local_dir) = request.local_dir else {
        // Cache layout: needs a digest to name the blob, so an unverifiable
        // file can only go to a plain local directory.
        let repo = hfd_core::hub::RepoRef::new(request.repo_type, request.repo_id);
        let revision = request
            .revision
            .unwrap_or(modelscope::DEFAULT_REVISION)
            .to_string();
        let mut placed = Vec::new();
        for file in &selected {
            let Some(digest) = &file.sha256 else {
                bail!(
                    "{} has no published sha256, so it cannot be placed in the cache; use --local-dir",
                    file.path
                );
            };
            let src = staging.join(&file.path);
            let snapshot = hfd_core::cache::place_verified(
                request.cache_root,
                &repo,
                &revision,
                &file.path,
                digest,
                &revision,
                &src,
            )
            .context("placing file in cache")?;
            placed.push(snapshot);
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(placed);
    };

    if request.flat {
        let mut placed = Vec::new();
        for path in fetched {
            let name = match path.file_name() {
                Some(name) => name.to_owned(),
                None => continue,
            };
            let target = local_dir.join(&name);
            if target != path {
                std::fs::rename(&path, &target).or_else(|_| {
                    std::fs::copy(&path, &target)?;
                    std::fs::remove_file(&path)
                })?;
            }
            placed.push(target);
        }
        Ok(placed)
    } else {
        Ok(fetched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// One-request HTTP stub: answers `listing` to the files call and `payload`
    /// to the file call, then exits.
    fn stub(listing: &str, payload: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let listing = listing.to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                if request_line.contains("/repo/files") {
                    let body = listing.as_bytes();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(body);
                } else {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(payload);
                }
            }
        });
        format!("http://{addr}")
    }

    fn listing(path: &str, sha: &str) -> String {
        format!(
            r#"{{"Code":200,"Success":true,"Data":{{"Files":[
                {{"Type":"blob","Path":"{path}","Size":3,"Sha256":"{sha}"}}]}}}}"#
        )
    }

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    async fn fetch(api: &str, dir: &Path, include: &[String]) -> Result<Vec<PathBuf>> {
        run_at(
            api,
            Request {
                repo_id: "acme/tiny",
                repo_type: RepoType::Model,
                revision: None,
                filenames: &[],
                include,
                exclude: &[],
                local_dir: Some(dir),
                flat: false,
                cache_root: dir,
                dry_run: false,
            },
        )
        .await
    }

    #[tokio::test]
    async fn downloads_and_verifies_a_file() {
        let api = stub(&listing("m.gguf", ABC_SHA), b"abc");
        let dir = tempfile::tempdir().unwrap();
        let paths = fetch(&api, dir.path(), &[]).await.unwrap();
        assert_eq!(paths, vec![dir.path().join("m.gguf")]);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"abc");
    }

    #[tokio::test]
    async fn a_bad_digest_removes_the_file_and_fails() {
        let api = stub(
            &listing("m.gguf", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            b"abc",
        );
        let dir = tempfile::tempdir().unwrap();
        let err = fetch(&api, dir.path(), &[]).await.unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("sha256 mismatch"), "got {chain}");
        assert!(!dir.path().join("m.gguf").exists());
    }

    #[tokio::test]
    async fn a_glob_that_matches_nothing_is_an_error() {
        let api = stub(&listing("m.gguf", ABC_SHA), b"abc");
        let dir = tempfile::tempdir().unwrap();
        let err = fetch(&api, dir.path(), &["*.safetensors".to_string()])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no files"), "got {err}");
    }

    #[tokio::test]
    async fn dry_run_lists_without_writing() {
        let api = stub(&listing("m.gguf", ABC_SHA), b"abc");
        let dir = tempfile::tempdir().unwrap();
        let paths = run_at(
            &api,
            Request {
                repo_id: "acme/tiny",
                repo_type: RepoType::Model,
                revision: None,
                filenames: &[],
                include: &[],
                exclude: &[],
                local_dir: Some(dir.path()),
                flat: false,
                cache_root: dir.path(),
                dry_run: true,
            },
        )
        .await
        .unwrap();
        assert!(paths.is_empty());
        assert!(!dir.path().join("m.gguf").exists());
    }
}
