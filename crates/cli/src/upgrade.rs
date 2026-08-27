//! Replace the running binary with the newest published release.
//!
//! `whycodes upgrade` previously printed instructions to re-clone and
//! `cargo install`, while `--help` advertised it as "Self-update". This does
//! what it says.
//!
//! Every download is verified against the release's `SHA256SUMS` before it is
//! allowed anywhere near the installed binary, and the replacement is done by
//! rename rather than by writing over the target — a download that dies
//! halfway must not leave a truncated executable where a working one was.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const REPO: &str = "whycorporation/whycodes";
const USER_AGENT: &str = concat!("whycodes/", env!("CARGO_PKG_VERSION"));

/// The release-artifact name for the platform this binary was built for.
///
/// Must match the `matrix.target` values in `.github/workflows/release.yml`;
/// the installers derive the same names from `uname`.
pub fn target_archive() -> Result<&'static str> {
    archive_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn archive_for(os: &str, arch: &str) -> Result<&'static str> {
    Ok(match (os, arch) {
        ("linux", "x86_64") => "whycodes-x86_64-unknown-linux-gnu.tar.gz",
        ("macos", "aarch64") => "whycodes-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "whycodes-x86_64-apple-darwin.tar.gz",
        ("windows", "x86_64") => "whycodes-x86_64-pc-windows-msvc.zip",
        (os, arch) => bail!(
            "no published binary for {os}/{arch} — build from source with `cargo build --release`"
        ),
    })
}

/// Compare dotted numeric versions. Returns true when `candidate` is newer.
///
/// Deliberately small: release tags here are `vMAJOR.MINOR.PATCH`. Anything it
/// cannot parse compares as not-newer, so a malformed tag never triggers a
/// download.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<Vec<u64>> {
        let v = v.trim().trim_start_matches('v');
        let out: Vec<u64> = v
            .split('.')
            .map(|p| p.split(['-', '+']).next().unwrap_or(p))
            .map(|p| p.parse().ok())
            .collect::<Option<Vec<u64>>>()?;
        (!out.is_empty()).then_some(out)
    }
    let (Some(a), Some(b)) = (parts(candidate), parts(current)) else {
        return false;
    };
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Find `name` in a `sha256sum`-format file and return its expected digest.
pub fn expected_digest(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start_matches('*').trim() == name).then(|| digest.trim().to_lowercase())
    })
}

pub(crate) fn digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Optional token for private repos (`GITHUB_TOKEN` or `GH_TOKEN`).
///
/// Public repos work without it. Private release assets only download via the
/// GitHub API when authenticated — browser download URLs return 404.
pub(crate) fn github_token() -> Option<String> {
    token_from_env(|k| std::env::var(k).ok())
}

pub(crate) fn token_from_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Some(v) = get(key)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    None
}

pub(crate) fn status_hint(status: u16, has_token: bool) -> &'static str {
    if status == 404 && !has_token {
        " (private repo? set GITHUB_TOKEN or GH_TOKEN, or publish the repository)"
    } else {
        ""
    }
}

pub(crate) fn asset_download_hint(status: u16, has_token: bool) -> &'static str {
    if status == 404 && !has_token {
        " (private repo? set GITHUB_TOKEN or GH_TOKEN)"
    } else {
        ""
    }
}

pub(crate) fn find_asset_id(release: &serde_json::Value, name: &str) -> Result<u64> {
    let assets = release["assets"]
        .as_array()
        .context("release has no assets array")?;
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(name))
        .with_context(|| format!("release has no asset named {name}"))?;
    asset["id"]
        .as_u64()
        .with_context(|| format!("asset {name} has no id"))
}

pub(crate) fn release_version(body: &serde_json::Value) -> Result<String> {
    let tag = body["tag_name"]
        .as_str()
        .context("release has no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

pub(crate) fn checksum_mismatch(name: &str, expected: &str, actual: &str) -> String {
    format!(
        "checksum mismatch for {name}\n  expected {expected}\n  actual   {actual}\nNothing was installed."
    )
}

pub(crate) async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    let mut req = client.get(url).header("User-Agent", USER_AGENT);
    if let Some(token) = github_token() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let response = req
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let hint = status_hint(status.as_u16(), github_token().is_some());
        bail!("{url} returned {status}{hint}");
    }
    Ok(response)
}

/// Download a release asset by name through the GitHub API.
///
/// Uses `Accept: application/octet-stream` on the asset id endpoint so private
/// releases work when a token is present (browser `/releases/download/` URLs
/// 404 on private repos even with a Bearer header).
async fn download_asset(
    client: &reqwest::Client,
    release: &serde_json::Value,
    name: &str,
) -> Result<Vec<u8>> {
    let id = find_asset_id(release, name)?;
    download_bytes(client, &release_asset_url(id), name).await
}

pub(crate) fn release_asset_url(id: u64) -> String {
    format!("https://api.github.com/repos/{REPO}/releases/assets/{id}")
}

pub(crate) async fn download_bytes(
    client: &reqwest::Client,
    url: &str,
    name: &str,
) -> Result<Vec<u8>> {
    let mut req = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/octet-stream");
    if let Some(token) = github_token() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let response = req
        .send()
        .await
        .with_context(|| format!("downloading asset {name}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let hint = asset_download_hint(status.as_u16(), github_token().is_some());
        bail!("asset {name} returned {status}{hint}");
    }
    Ok(response
        .bytes()
        .await
        .with_context(|| format!("reading asset {name}"))?
        .to_vec())
}

/// Ask GitHub for the latest release (metadata + asset list).
async fn latest_release_json(client: &reqwest::Client) -> Result<serde_json::Value> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    get(client, &url)
        .await?
        .json()
        .await
        .context("parsing latest release JSON")
}

/// Extract the `whycodes` executable from a downloaded archive.
pub(crate) fn extract(archive: &[u8], name: &str) -> Result<Vec<u8>> {
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .context("the downloaded archive is not a valid zip")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.name().ends_with("whycodes.exe") {
                let mut out = Vec::new();
                entry.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
        bail!("the archive did not contain whycodes.exe");
    }

    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("the archive is not a valid tar.gz")? {
        let mut entry = entry?;
        let is_binary = entry
            .path()
            .ok()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "whycodes"));
        if is_binary {
            let mut out = Vec::new();
            entry.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    bail!("the archive did not contain a whycodes binary");
}

/// Put `bytes` in place of the binary at `target`.
///
/// Writes a sibling file and renames it over the target. Windows refuses to
/// replace a running executable, so the current one is renamed aside first —
/// that succeeds while it is running, and the displaced file is removed on the
/// next upgrade.
pub fn replace_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target.parent().context("binary has no parent directory")?;
    let staged = dir.join(".whycodes.new");
    let displaced = dir.join(".whycodes.old");

    std::fs::write(&staged, bytes).with_context(|| format!("writing {}", staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }

    let _ = std::fs::remove_file(&displaced);
    if target.exists() && std::fs::rename(target, &displaced).is_err() {
        let _ = std::fs::remove_file(&staged);
        bail!(
            "could not move the current binary aside at {} — close any running whycodes and retry",
            target.display()
        );
    }

    if let Err(e) = std::fs::rename(&staged, target) {
        // Put the original back rather than leaving nothing installed.
        let _ = std::fs::rename(&displaced, target);
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| format!("installing to {}", target.display()));
    }

    let _ = std::fs::remove_file(&displaced);
    Ok(())
}

/// Where the running executable lives.
pub(crate) fn current_binary() -> Result<PathBuf> {
    std::env::current_exe().context("could not determine the path of the running binary")
}

/// Homebrew (and Linuxbrew) own the prefix; overwriting a Cellar binary
/// breaks `brew upgrade` / `brew doctor`. Install-script and cargo installs
/// are fine to self-update.
pub(crate) fn package_manager_upgrade_hint(path: &Path) -> Option<&'static str> {
    if looks_like_homebrew(path) {
        Some("this install is managed by Homebrew; update with `brew upgrade whycodes`")
    } else {
        None
    }
}

pub(crate) fn looks_like_homebrew(path: &Path) -> bool {
    if path_looks_like_homebrew(&path.to_string_lossy()) {
        return true;
    }
    // `brew` links `$(brew --prefix)/bin/whycodes` → Cellar. `current_exe`
    // often returns the symlink, not the resolved path.
    let Ok(target) = path.read_link() else {
        return false;
    };
    let resolved = if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or(path).join(target)
    };
    path_looks_like_homebrew(&resolved.to_string_lossy())
}

pub(crate) fn path_looks_like_homebrew(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.contains("/cellar/")
        || p.contains("/opt/homebrew/")
        || p.contains("/linuxbrew/")
        || p.contains("/homebrew/cellar/")
}

/// Run the upgrade. Returns the new version when one was installed.
pub async fn run() -> Result<Option<String>> {
    let target = current_binary()?;
    if let Some(hint) = package_manager_upgrade_hint(&target) {
        bail!("{hint}");
    }

    let current = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("building HTTP client")?;

    let body = latest_release_json(&client).await?;
    let version = release_version(&body)?;
    let archive_name = target_archive()?.to_string();
    let archive = download_asset(&client, &body, &archive_name).await?;
    let sums_bytes = download_asset(&client, &body, "SHA256SUMS").await?;
    let sums = String::from_utf8(sums_bytes).context("SHA256SUMS is not valid UTF-8")?;

    match decide_upgrade(current, &version, &sums, &archive_name, &archive)? {
        UpgradeDecision::UpToDate => Ok(None),
        UpgradeDecision::Install(binary) => {
            replace_binary(&target, &binary)?;
            Ok(Some(version))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UpgradeDecision {
    UpToDate,
    Install(Vec<u8>),
}

pub(crate) fn decide_upgrade(
    current: &str,
    version: &str,
    sums: &str,
    archive_name: &str,
    archive: &[u8],
) -> Result<UpgradeDecision> {
    if !is_newer(version, current) {
        return Ok(UpgradeDecision::UpToDate);
    }
    let expected = expected_digest(sums, archive_name)
        .with_context(|| format!("{archive_name} is not listed in SHA256SUMS"))?;
    let actual = digest_of(archive);
    if expected != actual {
        bail!("{}", checksum_mismatch(archive_name, &expected, &actual));
    }
    Ok(UpgradeDecision::Install(extract(archive, archive_name)?))
}

pub(crate) fn format_upgrade_outcome(
    current: &str,
    result: Result<Option<String>, String>,
) -> String {
    match result {
        Ok(Some(version)) => format!("Upgraded {current} → {version}"),
        Ok(None) => "Already on the latest release.".into(),
        Err(e) => e,
    }
}
