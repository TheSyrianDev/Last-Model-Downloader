//! Download queue for long-lived apps and the CLI daemon.
//!
//! A [`Queue`] holds jobs (a repo plus a list of files), runs them one at a
//! time by default, and persists its state to a JSON file so a restart
//! resumes unfinished work. Progress and state changes are sent on a
//! broadcast channel from [`Queue::subscribe`].
//!
//! Pause and cancel stop the active transfer. Partial files stay on disk, so a
//! later resume continues them. Cancel deletes partials only when asked to
//! (`delete_partial`), and only for jobs with a `local_dir`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinSet;

use crate::engine::{DownloadOptions, Downloader};
use crate::progress::{Event, EventSink};
use crate::{CancelToken, Config, Error, HubClient, RepoRef, RepoType, Result};

/// What to download. `paths` are repo-relative files at `revision`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    pub repo_type: RepoType,
    pub repo_id: String,
    pub revision: String,
    pub paths: Vec<String>,
    /// Real folder for the files. Required for `delete_partial` on cancel.
    #[serde(default)]
    pub local_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    Paused,
    Cancelled,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    pub spec: JobSpec,
    pub status: Status,
    pub created_at: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

/// Events from [`Queue::subscribe`]. Tags are stable JSON for GUI consumers.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueueEvent {
    Queued {
        id: String,
    },
    Started {
        id: String,
    },
    Progress {
        id: String,
        path: String,
        bytes: u64,
        total: u64,
        bytes_per_sec: f64,
        eta_secs: Option<u64>,
    },
    Paused {
        id: String,
    },
    Cancelled {
        id: String,
        deleted_partial: bool,
    },
    Finished {
        id: String,
        sha_ok: bool,
    },
    Failed {
        id: String,
        reason: String,
        retryable: bool,
    },
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    jobs: Vec<Job>,
    next_id: u64,
}

struct Inner {
    jobs: Vec<Job>,
    next_id: u64,
    max_parallel: usize,
    /// Cancel token of each running job, plus whether the stop was a pause.
    running: HashMap<String, (CancelToken, Stop)>,
    path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Pause,
    Cancel { delete_partial: bool },
}

/// Cheap to clone; all clones share one queue.
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Mutex<Inner>>,
    events: broadcast::Sender<QueueEvent>,
    wake: Arc<Notify>,
    hub: HubClient,
    options: DownloadOptions,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Queue {
    /// Opens (or creates) the queue at `state_file`. Jobs that were running
    /// when the process stopped go back to `Queued`, so [`Queue::run`] resumes
    /// them.
    pub fn open(
        state_file: impl Into<PathBuf>,
        config: Config,
        token: Option<String>,
        options: DownloadOptions,
    ) -> Result<Self> {
        let path = state_file.into();
        let mut persisted: Persisted = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| Error::Invalid(format!("queue state {}: {e}", path.display())))?,
            Err(_) => Persisted::default(),
        };
        for job in &mut persisted.jobs {
            if job.status == Status::Running {
                job.status = Status::Queued;
            }
        }
        let (events, _) = broadcast::channel(256);
        let queue = Self {
            inner: Arc::new(Mutex::new(Inner {
                jobs: persisted.jobs,
                next_id: persisted.next_id.max(1),
                max_parallel: 1,
                running: HashMap::new(),
                path,
            })),
            events,
            wake: Arc::new(Notify::new()),
            hub: HubClient::new(config, token)?,
            options,
        };
        queue.save()?;
        Ok(queue)
    }

    /// Sets how many jobs run at once. Default 1 (sequential).
    pub fn set_max_parallel(&self, n: usize) {
        self.inner.lock().unwrap().max_parallel = n.max(1);
        self.wake.notify_one();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.events.subscribe()
    }

    pub fn list(&self) -> Vec<Job> {
        self.inner.lock().unwrap().jobs.clone()
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.list().into_iter().find(|j| j.id == id)
    }

    /// Adds a job and returns its id.
    pub fn add(&self, spec: JobSpec) -> Result<String> {
        if spec.paths.is_empty() {
            return Err(Error::Invalid("job has no files".into()));
        }
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let id = format!("job-{}", inner.next_id);
            inner.next_id += 1;
            inner.jobs.push(Job {
                id: id.clone(),
                spec,
                status: Status::Queued,
                created_at: now(),
                error: None,
                retryable: false,
            });
            id
        };
        if let Err(error) = self.save() {
            // Undo the in-memory insert so a failed add is not run later and a
            // retry does not enqueue the same work twice.
            let mut inner = self.inner.lock().unwrap();
            inner.jobs.retain(|j| j.id != id);
            return Err(error);
        }
        let _ = self.events.send(QueueEvent::Queued { id: id.clone() });
        self.wake.notify_one();
        Ok(id)
    }

    /// Pauses a queued or running job. A running job keeps its partial files.
    pub fn pause(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let job = find_mut(&mut inner.jobs, id)?;
        match job.status {
            Status::Queued => job.status = Status::Paused,
            Status::Running => {
                if let Some((token, stop)) = inner.running.get_mut(id) {
                    *stop = Stop::Pause;
                    token.cancel();
                }
            }
            other => return Err(Error::Invalid(format!("cannot pause a {other:?} job"))),
        }
        drop(inner);
        self.save()?;
        let _ = self.events.send(QueueEvent::Paused { id: id.into() });
        Ok(())
    }

    /// Puts a paused job back in the queue.
    pub fn resume(&self, id: &str) -> Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            let job = find_mut(&mut inner.jobs, id)?;
            if job.status != Status::Paused {
                return Err(Error::Invalid(format!(
                    "cannot resume a {:?} job",
                    job.status
                )));
            }
            job.status = Status::Queued;
        }
        self.save()?;
        self.wake.notify_one();
        Ok(())
    }

    /// Cancels a job. With `delete_partial` and a `local_dir`, removes that
    /// job's unfinished files. Finished files are never deleted.
    pub fn cancel(&self, id: &str, delete_partial: bool) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let job = find_mut(&mut inner.jobs, id)?;
        match job.status {
            Status::Queued | Status::Paused => {
                job.status = Status::Cancelled;
                let spec = job.spec.clone();
                drop(inner);
                let deleted = delete_partial && remove_partials(&spec);
                self.save()?;
                let _ = self.events.send(QueueEvent::Cancelled {
                    id: id.into(),
                    deleted_partial: deleted,
                });
            }
            Status::Running => {
                if let Some((token, stop)) = inner.running.get_mut(id) {
                    *stop = Stop::Cancel { delete_partial };
                    token.cancel();
                }
            }
            other => return Err(Error::Invalid(format!("cannot cancel a {other:?} job"))),
        }
        Ok(())
    }

    /// Runs queued jobs. Returns only if the future is dropped, which also
    /// aborts any job still running (its partial files stay on disk, and the
    /// job is picked up again after reopening the queue). Spawn it on a tokio
    /// runtime, or select on it to stop with the process.
    pub async fn run(&self) {
        let mut tasks: JoinSet<()> = JoinSet::new();
        loop {
            while tasks.try_join_next().is_some() {}
            let next = {
                let mut inner = self.inner.lock().unwrap();
                if inner.running.len() >= inner.max_parallel {
                    None
                } else {
                    let job = inner.jobs.iter_mut().find(|j| j.status == Status::Queued);
                    job.map(|j| {
                        j.status = Status::Running;
                        j.clone()
                    })
                }
            };
            match next {
                Some(job) => {
                    let _ = self.save();
                    let token = CancelToken::new();
                    self.inner
                        .lock()
                        .unwrap()
                        .running
                        .insert(job.id.clone(), (token.clone(), Stop::Pause));
                    let queue = self.clone();
                    tasks.spawn(async move { queue.execute(job, token).await });
                }
                None if tasks.is_empty() => self.wake.notified().await,
                None => {
                    tokio::select! {
                        _ = self.wake.notified() => {}
                        _ = tasks.join_next() => {}
                    }
                }
            }
        }
    }

    async fn execute(&self, job: Job, token: CancelToken) {
        let _ = self.events.send(QueueEvent::Started { id: job.id.clone() });
        let outcome = self.download_all(&job, &token).await;
        let stop = self
            .inner
            .lock()
            .unwrap()
            .running
            .remove(&job.id)
            .map(|(_, stop)| stop)
            .unwrap_or(Stop::Pause);

        match outcome {
            Ok(sha_ok) => {
                self.set_status(&job.id, Status::Done, None, false);
                let _ = self.events.send(QueueEvent::Finished {
                    id: job.id.clone(),
                    sha_ok,
                });
            }
            Err(Error::Cancelled) => match stop {
                Stop::Pause => {
                    self.set_status(&job.id, Status::Paused, None, false);
                }
                Stop::Cancel { delete_partial } => {
                    self.set_status(&job.id, Status::Cancelled, None, false);
                    let deleted = delete_partial && remove_partials(&job.spec);
                    let _ = self.events.send(QueueEvent::Cancelled {
                        id: job.id.clone(),
                        deleted_partial: deleted,
                    });
                }
            },
            Err(error) => {
                let retryable = is_retryable(&error);
                let reason = error.to_string();
                self.set_status(&job.id, Status::Failed, Some(reason.clone()), retryable);
                let _ = self.events.send(QueueEvent::Failed {
                    id: job.id.clone(),
                    reason,
                    retryable,
                });
            }
        }
        let _ = self.save();
        self.wake.notify_one();
    }

    async fn download_all(&self, job: &Job, token: &CancelToken) -> Result<bool> {
        let repo = RepoRef::new(job.spec.repo_type, job.spec.repo_id.clone());
        let mut options = self.options.clone();
        options.local_dir = job.spec.local_dir.clone();
        let downloader = Downloader::new(self.hub.clone(), options);
        let events = self.events.clone();
        let id = job.id.clone();
        let sink: EventSink = Arc::new(move |event: Event| {
            if let Event::Progress {
                path,
                downloaded,
                total,
                bytes_per_sec,
                eta_secs,
            } = event
            {
                let _ = events.send(QueueEvent::Progress {
                    id: id.clone(),
                    path,
                    bytes: downloaded,
                    total,
                    bytes_per_sec,
                    eta_secs,
                });
            }
        });
        for path in &job.spec.paths {
            downloader
                .download_file_cancellable(&repo, path, &job.spec.revision, &sink, token)
                .await?;
        }
        Ok(true)
    }

    fn set_status(&self, id: &str, status: Status, error: Option<String>, retryable: bool) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
            job.status = status;
            job.error = error;
            job.retryable = retryable;
        }
    }

    fn save(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        let persisted = Persisted {
            version: 1,
            jobs: inner.jobs.clone(),
            next_id: inner.next_id,
        };
        let path = inner.path.clone();
        drop(inner);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        let bytes =
            serde_json::to_vec_pretty(&persisted).map_err(|e| Error::Invalid(e.to_string()))?;
        {
            // Flush the data before the rename, so a crash cannot leave the
            // renamed file empty or truncated.
            use std::io::Write;
            let mut file = fs::File::create(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

fn find_mut<'a>(jobs: &'a mut [Job], id: &str) -> Result<&'a mut Job> {
    jobs.iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| Error::Invalid(format!("no job {id}")))
}

fn is_retryable(error: &Error) -> bool {
    !matches!(error, Error::Invalid(_) | Error::Checksum { .. })
}

/// Removes `.hfd.incomplete` and `.hfd.state` files from the job's
/// `<local_dir>/.cache/huggingface/download/`. Returns true if any were removed.
fn remove_partials(spec: &JobSpec) -> bool {
    let Some(local_dir) = &spec.local_dir else {
        return false;
    };
    let dir: PathBuf = Path::new(local_dir).join(".cache/huggingface/download");
    let Ok(entries) = fs::read_dir(&dir) else {
        return false;
    };
    let mut removed = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".hfd.incomplete") || name.ends_with(".hfd.state") {
            removed |= fs::remove_file(entry.path()).is_ok();
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> JobSpec {
        JobSpec {
            repo_type: RepoType::Model,
            repo_id: "o/r".into(),
            revision: "main".into(),
            paths: vec!["m.gguf".into()],
            local_dir: None,
        }
    }

    fn queue(dir: &Path) -> Queue {
        Queue::open(
            dir.join("queue.json"),
            Config::from_env(),
            None,
            DownloadOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn add_pause_resume_cancel_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let q = queue(dir.path());
        let id = q.add(spec()).unwrap();
        q.pause(&id).unwrap();
        assert_eq!(q.get(&id).unwrap().status, Status::Paused);
        q.resume(&id).unwrap();
        assert_eq!(q.get(&id).unwrap().status, Status::Queued);
        q.cancel(&id, false).unwrap();
        assert_eq!(q.get(&id).unwrap().status, Status::Cancelled);
        assert!(q.resume(&id).is_err());
    }

    #[test]
    fn restart_turns_running_into_queued_and_keeps_ids() {
        let dir = tempfile::tempdir().unwrap();
        {
            let q = queue(dir.path());
            let id = q.add(spec()).unwrap();
            q.inner.lock().unwrap().jobs[0].status = Status::Running;
            q.save().unwrap();
            assert_eq!(id, "job-1");
        }
        let q = queue(dir.path());
        let job = q.get("job-1").unwrap();
        assert_eq!(job.status, Status::Queued);
        assert_eq!(q.add(spec()).unwrap(), "job-2");
    }

    #[test]
    fn failed_add_does_not_leave_a_phantom_job() {
        let dir = tempfile::tempdir().unwrap();
        let q = queue(dir.path());
        // Replace the state directory with a file so the next save() fails.
        let state_dir = dir.path();
        fs::remove_file(state_dir.join("queue.json")).ok();
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, b"x").unwrap();
        q.inner.lock().unwrap().path = blocked.join("queue.json");
        assert!(q.add(spec()).is_err());
        assert!(
            q.list().is_empty(),
            "failed add must not leave a queued job"
        );
    }

    #[test]
    fn cancel_delete_partial_removes_only_partials() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("out");
        let download = local.join(".cache/huggingface/download");
        fs::create_dir_all(&download).unwrap();
        fs::write(download.join("a.hfd.incomplete"), b"partial").unwrap();
        fs::write(download.join("a.hfd.state"), b"{}").unwrap();
        fs::write(local.join("finished.gguf"), b"done").unwrap();
        let q = queue(dir.path());
        let id = q
            .add(JobSpec {
                local_dir: Some(local.clone()),
                ..spec()
            })
            .unwrap();
        q.cancel(&id, true).unwrap();
        assert!(!download.join("a.hfd.incomplete").exists());
        assert!(!download.join("a.hfd.state").exists());
        assert!(local.join("finished.gguf").exists());
    }

    #[test]
    fn cancel_without_flag_keeps_partials() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("out");
        let download = local.join(".cache/huggingface/download");
        fs::create_dir_all(&download).unwrap();
        fs::write(download.join("a.hfd.incomplete"), b"partial").unwrap();
        let q = queue(dir.path());
        let id = q
            .add(JobSpec {
                local_dir: Some(local),
                ..spec()
            })
            .unwrap();
        q.cancel(&id, false).unwrap();
        assert!(download.join("a.hfd.incomplete").exists());
    }
}
