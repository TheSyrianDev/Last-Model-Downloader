//! `hfd lastengine`: fetch the Last Engine release asset for this machine and
//! check it against the release's SHA256SUMS.txt before keeping it.

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use hfd_core::{Config, hub::HubClient};
use sha2::{Digest, Sha256};

const REPO: &str = "TheSyrianDev/LastEngine";

#[derive(Debug, Args)]
pub struct LastEngineArgs {
    /// Folder to save the asset in (created if missing).
    #[arg(long, default_value = ".")]
    pub dest: PathBuf,
    /// Release tag to fetch, or `latest`.
    #[arg(long, default_value = "latest")]
    pub tag: String,
    /// Build variant. Only `cuda` is published today.
    #[arg(long, default_value = "cuda")]
    pub variant: String,
    /// Print what would be fetched and stop.
    #[arg(long)]
    pub dry_run: bool,
}

/// Asset name for this OS and CPU, e.g. `lastengine-linux-x86_64-cuda.tar.gz`.
pub fn asset_name(os: &str, arch: &str, variant: &str) -> Result<String> {
    let os_part = match os {
        "linux" => "linux",
        "windows" => "windows",
        other => bail!("no Last Engine build for {other} yet"),
    };
    if arch != "x86_64" {
        bail!("no Last Engine build for {arch} yet");
    }
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Ok(format!("lastengine-{os_part}-{arch}-{variant}.{ext}"))
}

/// Finds `name`'s checksum in a `sha256sum`-style file.
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn release_base(tag: &str) -> String {
    if tag == "latest" {
        format!("https://github.com/{REPO}/releases/latest/download")
    } else {
        format!("https://github.com/{REPO}/releases/download/{tag}")
    }
}

pub async fn run(args: LastEngineArgs) -> Result<()> {
    let name = asset_name(std::env::consts::OS, std::env::consts::ARCH, &args.variant)?;
    let base = release_base(&args.tag);
    if args.dry_run {
        println!("would fetch {base}/{name} and verify against {base}/SHA256SUMS.txt");
        return Ok(());
    }
    let hub = HubClient::new(Config::from_env(), None)?;
    let client = hub.http_client();

    let sums_url = format!("{base}/SHA256SUMS.txt");
    let sums = client
        .get(&sums_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("fetching {sums_url}"))?
        .text()
        .await?;
    let expected = expected_sha256(&sums, &name)
        .with_context(|| format!("{name} is not listed in SHA256SUMS.txt"))?;

    std::fs::create_dir_all(&args.dest)?;
    let target = args.dest.join(&name);
    let asset_url = format!("{base}/{name}");
    let mut response = client
        .get(&asset_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("fetching {asset_url}"))?;

    let partial = target.with_extension("partial");
    let mut file = std::fs::File::create(&partial)?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk)?;
    }
    file.sync_all()?;
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(&partial);
        bail!("SHA-256 mismatch for {name}: expected {expected}, got {actual}; file discarded");
    }
    std::fs::rename(&partial, &target)?;
    println!("{}", target.display());
    println!("sha256 {actual} (verified against SHA256SUMS.txt)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_asset_per_platform() {
        assert_eq!(
            asset_name("linux", "x86_64", "cuda").unwrap(),
            "lastengine-linux-x86_64-cuda.tar.gz"
        );
        assert_eq!(
            asset_name("windows", "x86_64", "cuda").unwrap(),
            "lastengine-windows-x86_64-cuda.zip"
        );
        assert!(asset_name("macos", "aarch64", "cuda").is_err());
        assert!(asset_name("linux", "aarch64", "cuda").is_err());
    }

    #[test]
    fn reads_checksum_for_named_asset() {
        let sums = "d28268f953dc9f62fb4f51b3332ad15ae5a09ce66ab28c5dff11486926f0cd5f  lastengine-linux-x86_64-cuda.tar.gz\nefda67bc7068ab5908c6c749bfa0aebf893eeb2ce4ce122cd74fdc2968d3416f  *lastengine-windows-x86_64-cuda.zip\n";
        assert_eq!(
            expected_sha256(sums, "lastengine-linux-x86_64-cuda.tar.gz").unwrap(),
            "d28268f953dc9f62fb4f51b3332ad15ae5a09ce66ab28c5dff11486926f0cd5f"
        );
        assert_eq!(
            expected_sha256(sums, "lastengine-windows-x86_64-cuda.zip").unwrap(),
            "efda67bc7068ab5908c6c749bfa0aebf893eeb2ce4ce122cd74fdc2968d3416f"
        );
        assert!(expected_sha256(sums, "missing.tar.gz").is_none());
    }
}
