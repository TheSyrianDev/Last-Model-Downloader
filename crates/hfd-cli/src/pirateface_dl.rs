//! `hfd download --source pirateface`: opt-in transfer through Pirate Face.
//! Lookup over HTTP, torrent transfer through `aria2c`, then sha256 verification.
//! Never falls back to Hugging Face.

use std::path::Path;

use anyhow::{Context, Result, bail};
use hfd_core::pirateface::{self, AriaTorrent};

pub async fn run(
    repo_id: &str,
    revision: Option<&str>,
    filenames: &[String],
    local_dir: Option<&Path>,
) -> Result<()> {
    let Some(dest) = local_dir else {
        bail!("--source pirateface needs --local-dir; the HF cache layout is not supported yet");
    };
    let entry = match pirateface::lookup(repo_id)
        .await
        .context("Pirate Face lookup")?
    {
        Some(entry) => entry,
        None => bail!("{repo_id} is not listed on Pirate Face"),
    };
    if !entry.hf {
        bail!(
            "{repo_id} is a Pirate Face-only entry (no Hugging Face commit); this is not supported yet"
        );
    }
    if let Some(wanted) = revision
        && wanted != entry.revision
    {
        bail!(
            "revision mismatch: Pirate Face has {}, you asked for {wanted}",
            entry.revision
        );
    }
    let backend = AriaTorrent {
        bin: "aria2c".into(),
    };
    let paths = pirateface::fetch_verified(&backend, &entry, filenames, dest)
        .context("Pirate Face transfer")?;
    for path in paths {
        println!("{}", path.display());
    }
    Ok(())
}
