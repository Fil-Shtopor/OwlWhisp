//! GitHub release discovery and verified Windows installer downloads.
//! No installer runs until the user requests it. Audio and settings are never sent.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, TryRecvError, bounded};
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const REPOSITORY: &str = "Fil-Shtopor/OwlWhisp";
pub const RELEASES_PAGE: &str = "https://github.com/Fil-Shtopor/OwlWhisp/releases";
const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preferences {
    pub automatic_checks: bool,
    pub include_previews: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            automatic_checks: true,
            include_previews: true,
        }
    }
}

impl Preferences {
    pub fn load() -> Self {
        std::fs::read(crate::paths::app_data_dir().join("update-preferences.json"))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = crate::paths::app_data_dir().join("update-preferences.json");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(temporary, path).map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Available {
    pub version: Version,
    pub preview: bool,
    pub page: String,
    installer: Option<Asset>,
    checksums: Option<Asset>,
}

impl Available {
    pub fn can_install(&self) -> bool {
        self.installer.is_some() && self.checksums.is_some()
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    html_url: String,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug)]
pub struct Downloaded {
    pub version: Version,
    path: PathBuf,
    sha256: String,
}

/// A bounded background job, active only while checking or downloading.
pub struct Job<T> {
    result: Receiver<Result<T, String>>,
    cancel: Arc<AtomicBool>,
    downloaded: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

impl<T> Job<T> {
    pub fn poll(&self) -> Option<Result<T, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("Update worker stopped unexpectedly".into())),
        }
    }
    pub fn progress(&self) -> (u64, u64) {
        (
            self.downloaded.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn job<T: Send + 'static>(
    work: impl FnOnce(Arc<AtomicBool>, Arc<AtomicU64>, Arc<AtomicU64>) -> Result<T, String> + Send + 'static,
) -> Job<T> {
    let (sender, result) = bounded(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let downloaded = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let handles = (Arc::clone(&cancel), Arc::clone(&downloaded), Arc::clone(&total));
    std::thread::Builder::new()
        .name("owlwhisp-update".into())
        .spawn(move || {
            let outcome = work(handles.0, handles.1, handles.2);
            let _ = sender.send(outcome);
        })
        .expect("start update worker");
    Job {
        result,
        cancel,
        downloaded,
        total,
    }
}

fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("OwlWhisp/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("Too many download redirects");
            }
            let allowed = matches!(
                attempt.url().host_str(),
                Some(
                    "github.com"
                        | "api.github.com"
                        | "release-assets.githubusercontent.com"
                        | "objects.githubusercontent.com"
                )
            );
            if attempt.url().scheme() == "https" && allowed {
                attempt.follow()
            } else {
                attempt.error("Update redirect left GitHub's HTTPS download servers")
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}

fn limited_get(client: &Client, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    if response.content_length().is_some_and(|len| len > limit) {
        return Err("Update metadata is too large".into());
    }
    let mut data = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > limit {
        return Err("Update metadata is too large".into());
    }
    Ok(data)
}

fn windows_assets(version: &Version, arch: &str) -> Option<(String, String)> {
    let (target, checksums) = match arch {
        "x86_64" => ("x86_64-pc-windows-msvc", "SHA256SUMS-windows-x64.txt"),
        "aarch64" => ("aarch64-pc-windows-msvc", "SHA256SUMS-windows-arm64.txt"),
        _ => return None,
    };
    Some((format!("OwlWhisp-{version}-{target}-setup.exe"), checksums.into()))
}

fn trusted_asset(asset: &Asset, tag: &str, name: &str) -> bool {
    asset.name == name
        && asset.size > 0
        && asset.size <= MAX_INSTALLER
        && asset.browser_download_url
            == format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}")
}

fn select_release(
    releases: Vec<Release>,
    current: &Version,
    previews: bool,
    os: &str,
    arch: &str,
) -> Option<Available> {
    releases
        .into_iter()
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
            if release.draft
                || version <= *current
                || (!previews && (release.prerelease || !version.pre.is_empty()))
            {
                return None;
            }
            let page = format!(
                "https://github.com/{REPOSITORY}/releases/tag/{}",
                release.tag_name
            );
            if release.html_url != page {
                return None;
            }
            let names = (os == "windows")
                .then(|| windows_assets(&version, arch))
                .flatten();
            let (installer, checksums) = if let Some((name, checksum_name)) = names {
                let mut installers = release
                    .assets
                    .iter()
                    .filter(|a| trusted_asset(a, &release.tag_name, &name));
                let installer = installers.next().cloned();
                if installers.next().is_some() {
                    return None;
                }
                let mut sums = release
                    .assets
                    .iter()
                    .filter(|a| trusted_asset(a, &release.tag_name, &checksum_name) && a.size <= 128 * 1024);
                let checksum = sums.next().cloned();
                if sums.next().is_some() || installer.is_none() || checksum.is_none() {
                    return None;
                }
                (installer, checksum)
            } else {
                (None, None)
            };
            Some(Available {
                version,
                preview: release.prerelease,
                page,
                installer,
                checksums,
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

pub fn check(current: &str, previews: bool) -> Job<Option<Available>> {
    let current = current.to_string();
    job(move |_, _, _| {
        let current = Version::parse(&current).map_err(|e| e.to_string())?;
        let client = client()?;
        let data = limited_get(
            &client,
            &format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"),
            MAX_METADATA,
        )?;
        let releases = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
        Ok(select_release(
            releases,
            &current,
            previews,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ))
    })
}

fn checksum(text: &str, filename: &str) -> Result<String, String> {
    let mut matching = text.lines().filter_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == filename).then_some(hash)
    });
    let hash = matching.next().ok_or("Installer checksum is missing")?;
    if matching.next().is_some() || hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Installer checksum is invalid or duplicated".into());
    }
    Ok(hash.to_ascii_lowercase())
}

fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if hex::encode(hash.finalize()) != expected {
        return Err("Update checksum mismatch; installer will not run".into());
    }
    Ok(())
}

pub fn download(available: &Available) -> Job<Downloaded> {
    download_into(available, crate::paths::app_data_dir().join("updates"))
}

fn download_into(available: &Available, destination: PathBuf) -> Job<Downloaded> {
    let available = available.clone();
    job(move |cancel, downloaded, total| {
        let installer = available
            .installer
            .ok_or("No in-app installer for this platform")?;
        let checksums = available.checksums.ok_or("Release checksums are missing")?;
        let client = client()?;
        let sums = limited_get(&client, &checksums.browser_download_url, 128 * 1024)?;
        let expected = checksum(
            std::str::from_utf8(&sums).map_err(|e| e.to_string())?,
            &installer.name,
        )?;
        if installer
            .digest
            .as_ref()
            .is_some_and(|digest| digest != &format!("sha256:{expected}"))
        {
            return Err("GitHub digest disagrees with the release checksum".into());
        }
        let directory = destination.join(available.version.to_string());
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(&installer.name);
        let partial = path.with_extension("exe.part");
        let result = (|| {
            let mut response = client
                .get(&installer.browser_download_url)
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            if response.content_length().is_some_and(|len| len != installer.size) {
                return Err("Installer size disagrees with GitHub metadata".into());
            }
            total.store(installer.size, Ordering::Relaxed);
            let mut file = File::create(&partial).map_err(|e| e.to_string())?;
            let mut buffer = [0u8; 64 * 1024];
            let mut count = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Update download cancelled".into());
                }
                let read = response.read(&mut buffer).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                count += read as u64;
                if count > installer.size {
                    return Err("Installer is larger than expected".into());
                }
                file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
                downloaded.store(count, Ordering::Relaxed);
            }
            if count != installer.size {
                return Err("Installer download is incomplete".into());
            }
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            verify_file(&partial, &expected)?;
            if cancel.load(Ordering::Relaxed) {
                return Err("Update download cancelled".into());
            }
            std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
            mark_internet_download(&path, &installer.browser_download_url)?;
            Ok(Downloaded {
                version: available.version,
                path,
                sha256: expected,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(partial);
        }
        result
    })
}

fn mark_internet_download(path: &Path, source: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        // Keep the same provenance used by browser downloads so Windows can check
        // the installer normally. Do not mark it as a local/trusted-zone file.
        let mut zone_path = path.as_os_str().to_os_string();
        zone_path.push(":Zone.Identifier");
        std::fs::write(
            zone_path,
            format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={source}\r\n"),
        )
        .map_err(|e| format!("Could not retain Windows download provenance: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, source);
        Ok(())
    }
}

#[cfg(windows)]
fn verify_publisher(current: &Path, installer: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // A signed app must not update to an unsigned file or a different publisher.
    // An unsigned legacy app can migrate to the first signed release.
    let script = r#"$ErrorActionPreference='Stop';
        $current=Get-AuthenticodeSignature -LiteralPath $env:OWLWHISP_CURRENT_EXE;
        if ($current.Status -eq 'NotSigned') { exit 0 };
        if ($current.Status -ne 'Valid') { throw 'Installed application signature is invalid' };
        $next=Get-AuthenticodeSignature -LiteralPath $env:OWLWHISP_UPDATE_EXE;
        if ($next.Status -ne 'Valid' -or -not $next.TimeStamperCertificate) { throw 'Update requires a trusted, timestamped signature' };
        if ($current.SignerCertificate.Subject -ne $next.SignerCertificate.Subject) { throw 'Update publisher differs from the installed application' }"#;
    let shell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or("Windows system directory is unavailable")?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = std::process::Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("OWLWHISP_CURRENT_EXE", current)
        .env("OWLWHISP_UPDATE_EXE", installer)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output().map_err(|e| format!("Could not verify update publisher: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "Update publisher verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Opens the installer wizard after arranging a wait for this process to exit.
/// The caller quits only after this succeeds. NSIS preserves per-user model/settings data.
pub fn launch_installer(downloaded: &Downloaded) -> Result<(), String> {
    verify_file(&downloaded.path, &downloaded.sha256)?;
    #[cfg(windows)]
    {
        if !packaged_windows_app() {
            return Err("Install a packaged Windows build before using in-app installation".into());
        }
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let directory = executable.parent().ok_or("Application directory is missing")?;
        let destination = directory.to_str().ok_or("Application path is not Unicode")?;
        if destination.chars().any(|c| c == '"' || c.is_control()) {
            return Err("Application path cannot be passed to the installer".into());
        }
        verify_publisher(&executable, &downloaded.path)?;
        // NSIS requires /D to be last and unquoted, including paths with spaces.
        lw_platform::updates::open_installer(
            &downloaded.path,
            &format!("/UPDATE_PID={} /D={destination}", std::process::id()),
        )
    }
    #[cfg(not(windows))]
    Err("In-app installation is currently supported on Windows x64/ARM64".into())
}

/// A source-build output directory must never be overwritten by a release installer.
pub fn packaged_windows_app() -> bool {
    if !cfg!(windows) {
        return false;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::read(exe.parent()?.join("BUILD_INFO.json")).ok())
        .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        .is_some_and(|info| {
            info["version"] == env!("CARGO_PKG_VERSION")
                && info["platform"]
                    .as_str()
                    .is_some_and(|platform| platform == "win-x64" || platform == "win-arm64")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, arch: &str) -> Release {
        let parsed = Version::parse(version).unwrap();
        let (name, sums) = windows_assets(&parsed, arch).unwrap();
        let asset = |name: String| Asset {
            browser_download_url: format!(
                "https://github.com/{REPOSITORY}/releases/download/v{version}/{name}"
            ),
            name,
            size: 100,
            digest: None,
        };
        Release {
            tag_name: format!("v{version}"),
            draft: false,
            prerelease: true,
            html_url: format!("https://github.com/{REPOSITORY}/releases/tag/v{version}"),
            assets: vec![asset(name), asset(sums)],
        }
    }

    #[test]
    fn selects_newest_complete_preview_and_matching_native_architecture() {
        let current = Version::new(0, 1, 1);
        let releases = vec![
            release("0.1.2", "aarch64"),
            release("0.1.3", "x86_64"),
            release("0.1.1", "aarch64"),
        ];
        let selected = select_release(releases, &current, true, "windows", "aarch64").unwrap();
        assert_eq!(selected.version, Version::new(0, 1, 2));
        assert!(selected.installer.unwrap().name.contains("aarch64"));
    }

    #[test]
    fn rejects_drafts_wrong_repository_urls_incomplete_releases_and_downgrades() {
        let current = Version::new(0, 1, 1);
        let mut draft = release("0.1.5", "x86_64");
        draft.draft = true;
        let mut external = release("0.1.4", "x86_64");
        external.assets[0]
            .browser_download_url
            .replace_range(.., "https://example.com/installer.exe");
        let mut incomplete = release("0.1.3", "x86_64");
        incomplete.assets.pop();
        assert!(
            select_release(
                vec![draft, external, incomplete, release("0.1.0", "x86_64")],
                &current,
                true,
                "windows",
                "x86_64"
            )
            .is_none()
        );
        assert!(
            select_release(
                vec![release("0.1.2", "x86_64")],
                &current,
                false,
                "windows",
                "x86_64"
            )
            .is_none()
        );
    }

    #[test]
    fn checksums_require_one_valid_entry_for_the_exact_installer() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  setup.exe\n"), "setup.exe").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash}  other.exe\n"), "setup.exe").is_err());
        assert!(checksum("bad  setup.exe", "setup.exe").is_err());
        assert!(checksum(&format!("{hash}  setup.exe\n{hash}  setup.exe"), "setup.exe").is_err());
    }

    #[test]
    fn rejects_an_installer_modified_after_download() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("setup.exe");
        std::fs::write(&path, b"verified installer fixture").unwrap();
        let expected = hex::encode(Sha256::digest(b"verified installer fixture"));
        verify_file(&path, &expected).unwrap();
        std::fs::write(&path, b"changed installer fixture").unwrap();
        assert!(verify_file(&path, &expected).is_err());
    }

    #[test]
    #[ignore = "Downloads a public GitHub release installer; never launches it"]
    fn public_github_installer_download_is_verified_without_installing() {
        fn wait<T>(job: Job<T>) -> T {
            let start = std::time::Instant::now();
            loop {
                if let Some(result) = job.poll() {
                    return result.unwrap();
                }
                assert!(start.elapsed() < Duration::from_secs(330), "Update job timed out");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let selected = wait(check("0.0.0", true)).expect("Published release");
        assert!(selected.can_install());
        let directory = tempfile::tempdir().unwrap();
        let downloaded = wait(download_into(&selected, directory.path().to_path_buf()));
        assert_eq!(downloaded.version, selected.version);
        assert!(downloaded.path.starts_with(directory.path()));
        verify_file(&downloaded.path, &downloaded.sha256).unwrap();
        #[cfg(windows)]
        {
            let mut ads = downloaded.path.as_os_str().to_os_string();
            ads.push(":Zone.Identifier");
            assert!(std::fs::read_to_string(ads).unwrap().contains("ZoneId=3"));
        }
        assert!(!downloaded.path.with_extension("exe.part").exists());
        println!(
            "Verified official installer {} ({} bytes), without executing it",
            downloaded.version,
            std::fs::metadata(downloaded.path).unwrap().len()
        );
    }
}
