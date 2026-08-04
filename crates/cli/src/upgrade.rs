//! Replace the running binary with the newest published release.
//!
//! `whycode upgrade` previously printed instructions to re-clone and
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

const REPO: &str = "whycorporation/whycode";
const USER_AGENT: &str = concat!("whycode/", env!("CARGO_PKG_VERSION"));

/// The release-artifact name for the platform this binary was built for.
///
/// Must match the `matrix.target` values in `.github/workflows/release.yml`;
/// the installers derive the same names from `uname`.
pub fn target_archive() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "whycode-x86_64-unknown-linux-gnu.tar.gz",
        ("macos", "aarch64") => "whycode-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "whycode-x86_64-apple-darwin.tar.gz",
        ("windows", "x86_64") => "whycode-x86_64-pc-windows-msvc.zip",
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

fn digest_of(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Optional token for private repos (`GITHUB_TOKEN` or `GH_TOKEN`).
///
/// Public repos work without it. Private release assets only download via the
/// GitHub API when authenticated — browser download URLs return 404.
fn github_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(key)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    None
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
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
        let hint = if status.as_u16() == 404 && github_token().is_none() {
            " (private repo? set GITHUB_TOKEN or GH_TOKEN, or publish the repository)"
        } else {
            ""
        };
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
    let assets = release["assets"]
        .as_array()
        .context("release has no assets array")?;
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(name))
        .with_context(|| format!("release has no asset named {name}"))?;
    let id = asset["id"]
        .as_u64()
        .with_context(|| format!("asset {name} has no id"))?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/assets/{id}");
    let mut req = client
        .get(&url)
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
        let hint = if status.as_u16() == 404 && github_token().is_none() {
            " (private repo? set GITHUB_TOKEN or GH_TOKEN)"
        } else {
            ""
        };
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

/// Extract the `whycode` executable from a downloaded archive.
fn extract(archive: &[u8], name: &str) -> Result<Vec<u8>> {
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .context("the downloaded archive is not a valid zip")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.name().ends_with("whycode.exe") {
                let mut out = Vec::new();
                entry.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
        bail!("the archive did not contain whycode.exe");
    }

    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().context("the archive is not a valid tar.gz")? {
        let mut entry = entry?;
        let is_binary = entry
            .path()
            .ok()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "whycode"));
        if is_binary {
            let mut out = Vec::new();
            entry.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    bail!("the archive did not contain a whycode binary");
}

/// Put `bytes` in place of the binary at `target`.
///
/// Writes a sibling file and renames it over the target. Windows refuses to
/// replace a running executable, so the current one is renamed aside first —
/// that succeeds while it is running, and the displaced file is removed on the
/// next upgrade.
pub fn replace_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target.parent().context("binary has no parent directory")?;
    let staged = dir.join(".whycode.new");
    let displaced = dir.join(".whycode.old");

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
            "could not move the current binary aside at {} — close any running whycode and retry",
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
fn current_binary() -> Result<PathBuf> {
    std::env::current_exe().context("could not determine the path of the running binary")
}

/// Run the upgrade. Returns the new version when one was installed.
pub async fn run() -> Result<Option<String>> {
    let current = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("building HTTP client")?;

    let body = latest_release_json(&client).await?;
    let tag = body["tag_name"]
        .as_str()
        .context("release has no tag_name")?
        .to_string();
    let version = tag.trim_start_matches('v').to_string();
    let archive_name = target_archive()?.to_string();

    if !is_newer(&version, current) {
        return Ok(None);
    }

    let archive = download_asset(&client, &body, &archive_name).await?;
    let sums_bytes = download_asset(&client, &body, "SHA256SUMS").await?;
    let sums = String::from_utf8(sums_bytes).context("SHA256SUMS is not valid UTF-8")?;

    let expected = expected_digest(&sums, &archive_name)
        .with_context(|| format!("{archive_name} is not listed in SHA256SUMS"))?;
    let actual = digest_of(&archive);
    if expected != actual {
        bail!(
            "checksum mismatch for {archive_name}\n  expected {expected}\n  actual   {actual}\nNothing was installed."
        );
    }

    let binary = extract(&archive, &archive_name)?;
    replace_binary(&current_binary()?, &binary)?;
    Ok(Some(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_compare_greater() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.1.1", "0.1.0"));
    }

    #[test]
    fn the_same_or_older_versions_do_not() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn shorter_versions_pad_with_zeros() {
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn an_unparseable_version_never_triggers_a_download() {
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.1.x", "0.1.0"));
    }

    #[test]
    fn prerelease_suffixes_compare_on_the_numeric_part() {
        assert!(is_newer("0.2.0-rc1", "0.1.0"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
    }

    #[test]
    fn finds_a_digest_in_sha256sums_format() {
        let sums = "\
abc123  whycode-x86_64-unknown-linux-gnu.tar.gz
def456  whycode-x86_64-pc-windows-msvc.zip
";
        assert_eq!(
            expected_digest(sums, "whycode-x86_64-pc-windows-msvc.zip"),
            Some("def456".to_string())
        );
        assert_eq!(expected_digest(sums, "not-listed.tar.gz"), None);
    }

    #[test]
    fn handles_the_binary_marker_sha256sum_writes() {
        assert_eq!(
            expected_digest("abc123 *whycode.tar.gz", "whycode.tar.gz"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn digests_are_lowercase_hex() {
        assert_eq!(
            digest_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn every_supported_platform_has_an_archive_name() {
        // The build this test runs in is by definition a supported platform.
        let name = target_archive().expect("this platform should be supported");
        assert!(name.starts_with("whycode-"));
        assert!(name.ends_with(".tar.gz") || name.ends_with(".zip"));
    }

    #[test]
    fn replacing_a_binary_is_atomic_from_the_readers_point_of_view() {
        let dir = std::env::temp_dir().join(format!("whycode-upgrade-{}", stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("whycode");
        std::fs::write(&target, b"old").unwrap();

        replace_binary(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        // No staging or displaced files left behind.
        assert!(!dir.join(".whycode.new").exists());
        assert!(!dir.join(".whycode.old").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_works_when_nothing_is_installed_yet() {
        let dir = std::env::temp_dir().join(format!("whycode-upgrade-{}", stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("whycode");

        replace_binary(&target, b"fresh").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"fresh");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn stamp() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    }
}
