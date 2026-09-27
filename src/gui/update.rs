//! Update check and in-app update from the project's GitHub releases.
//!
//! The check reads the latest release from the GitHub API. Updating downloads this platform's
//! package, verifies it against the release's `.sha256` file and unpacks it with the system's
//! `tar` (Windows 10 and later include one that also reads zip files). With the service installed,
//! the new package's `gpu-fanctl reinstall` runs elevated and puts both programs in the install
//! folder; otherwise the programs next to this app are replaced. Then the new app starts and this
//! one closes.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::{env, fs, thread};

use eframe::egui;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

#[cfg(windows)]
const PACKAGE_SUFFIX: &str = "-windows-x64.zip";
#[cfg(not(windows))]
const PACKAGE_SUFFIX: &str = "-linux-x64.tar.gz";
#[cfg(windows)]
const PROGRAMS: [&str; 2] = ["gpu-fanctl.exe", "gpu-fanctl-gui.exe"];
#[cfg(not(windows))]
const PROGRAMS: [&str; 2] = ["gpu-fanctl", "gpu-fanctl-gui"];

/// A published release newer than this app.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    /// The release notes (Markdown).
    pub notes: String,
    /// The release page on GitHub.
    pub page: String,
    /// This platform's package and its checksum file, if the release has them.
    package: Option<(Asset, Asset)>,
}

impl Release {
    pub fn has_package(&self) -> bool {
        self.package.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Asset {
    name: String,
    url: String,
    size: u64,
}

#[derive(Clone, Debug)]
pub enum Check {
    Checking,
    UpToDate { latest: String },
    Available(Release),
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum Install {
    /// Share downloaded, 0 to 1.
    Downloading(f32),
    Verifying,
    Unpacking,
    /// Waiting for the administrator prompt and the reinstall.
    Installing,
    Restarting,
    Failed(String),
}

#[derive(Default)]
pub struct Updater {
    check: Arc<Mutex<Option<Check>>>,
    install: Arc<Mutex<Option<Install>>>,
}

fn get<T: Clone>(state: &Mutex<Option<T>>) -> Option<T> {
    state.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set<T>(state: &Mutex<Option<T>>, value: T, ctx: &egui::Context) {
    *state.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
    ctx.request_repaint();
}

impl Updater {
    pub fn check_state(&self) -> Option<Check> {
        get(&self.check)
    }

    pub fn install_state(&self) -> Option<Install> {
        get(&self.install)
    }

    /// The release to update to, once a check found one.
    pub fn available(&self) -> Option<Release> {
        match self.check_state() {
            Some(Check::Available(release)) => Some(release),
            _ => None,
        }
    }

    pub fn is_checking(&self) -> bool {
        matches!(self.check_state(), Some(Check::Checking))
    }

    /// Whether an update is being downloaded or installed.
    pub fn is_installing(&self) -> bool {
        self.install_state().is_some_and(|s| !matches!(s, Install::Failed(_)))
    }

    /// Looks for a newer release in the background.
    pub fn check(&self, ctx: &egui::Context) {
        set(&self.check, Check::Checking, ctx);
        let (state, ctx) = (Arc::clone(&self.check), ctx.clone());
        thread::spawn(move || {
            let result = match latest_release() {
                Ok(release) if is_newer(&release.version, CURRENT) => Check::Available(release),
                Ok(release) => Check::UpToDate { latest: release.version },
                Err(e) => Check::Failed(e),
            };
            set(&state, result, &ctx);
        });
    }

    /// Downloads, verifies and installs `release`, then starts the new app and closes this one.
    pub fn install(&self, release: Release, ctx: &egui::Context) {
        set(&self.install, Install::Downloading(0.0), ctx);
        let (state, ctx) = (Arc::clone(&self.install), ctx.clone());
        thread::spawn(move || match install(&release, &|step| set(&state, step, &ctx)) {
            Ok(gui) => {
                set(&state, Install::Restarting, &ctx);
                match Command::new(&gui).spawn() {
                    Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => set(&state, Install::Failed(format!("Updated, but {} didn't start: {e}", gui.display())), &ctx),
                }
            }
            Err(e) => set(&state, Install::Failed(e), &ctx),
        });
    }
}

/// `1.2.3` from `v1.2.3` or `1.2.3-beta`, as numbers.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().user_agent(format!("gpu-fanctl-gui/{CURRENT}")).build().into()
}

fn latest_release() -> Result<Release, String> {
    let api = format!("{}/releases/latest", REPOSITORY.replacen("https://github.com/", "https://api.github.com/repos/", 1));
    let text = agent()
        .get(&api)
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|mut response| response.body_mut().read_to_string())
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let release: GitHubRelease = serde_json::from_str(&text).map_err(|e| format!("Unexpected answer from GitHub: {e}"))?;
    parse_version(&release.tag_name).ok_or_else(|| format!("The latest release has no version number: {}", release.tag_name))?;
    Ok(from_github(release))
}

fn from_github(release: GitHubRelease) -> Release {
    let asset = |a: &GitHubAsset| Asset { name: a.name.clone(), url: a.browser_download_url.clone(), size: a.size };
    let package = release.assets.iter().find(|a| a.name.ends_with(PACKAGE_SUFFIX)).and_then(|package| {
        let checksum = release.assets.iter().find(|a| a.name == format!("{}.sha256", package.name))?;
        Some((asset(package), asset(checksum)))
    });
    Release {
        version: release.tag_name.trim_start_matches('v').to_string(),
        notes: release.body.unwrap_or_default(),
        page: release.html_url,
        package,
    }
}

fn download(asset: &Asset, progress: &dyn Fn(f32)) -> Result<Vec<u8>, String> {
    let failed = |e: &dyn std::fmt::Display| format!("Downloading {} failed: {e}", asset.name);
    let mut response = agent().get(&asset.url).call().map_err(|e| failed(&e))?;
    let mut reader = response.body_mut().as_reader();
    let mut data = Vec::with_capacity(asset.size as usize);
    let mut chunk = vec![0; 256 * 1024];
    loop {
        let read = reader.read(&mut chunk).map_err(|e| failed(&e))?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..read]);
        progress(data.len() as f32 / asset.size.max(1) as f32);
    }
    Ok(data)
}

/// Lowercase hex SHA-256 of `data`.
fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks `data` against a `sha256sum` line (`<hex>  <name>`).
fn verify(data: &[u8], checksum_file: &str) -> Result<(), String> {
    let expected = checksum_file.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if expected.len() != 64 {
        return Err("The checksum file is not in the expected format.".into());
    }
    if sha256(data) != expected {
        return Err("The download doesn't match its checksum. Nothing was changed; try again.".into());
    }
    Ok(())
}

/// The system `tar`. On Windows the one in System32 (bsdtar), which unpacks zip files; another
/// `tar` on PATH, such as Git's, may not.
fn tar() -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let system = env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let mut command = Command::new(system.join("System32").join("tar.exe"));
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    Command::new("tar")
}

/// Unpacks `archive` into `into` and returns the package folder with both programs.
fn unpack(archive: &Path, into: &Path, package: &str) -> Result<PathBuf, String> {
    let output = tar()
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .map_err(|e| format!("Could not run tar to unpack the update: {e}"))?;
    if !output.status.success() {
        return Err(format!("Unpacking the update failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let folder = into.join(package.trim_end_matches(".zip").trim_end_matches(".tar.gz"));
    match PROGRAMS.iter().find(|p| !folder.join(p).is_file()) {
        Some(missing) => Err(format!("The update package has no {missing}.")),
        None => Ok(folder),
    }
}

/// Puts `name` from `from` into `to`. The old file is renamed first: a running program can be
/// renamed but not overwritten on Windows. `cleanup` removes the renamed file later.
fn replace(from: &Path, to: &Path, name: &str) -> Result<(), String> {
    let target = to.join(name);
    let old = to.join(format!("{name}.old"));
    let _ = fs::remove_file(&old);
    if target.exists() {
        fs::rename(&target, &old).map_err(|e| format!("Could not replace {}: {e}", target.display()))?;
    }
    fs::copy(from.join(name), &target).map(|_| ()).map_err(|e| {
        let _ = fs::rename(&old, &target);
        format!("Could not write {}: {e}", target.display())
    })
}

/// Removes programs renamed by an earlier update, once they no longer run.
pub fn cleanup() {
    if let Some(dir) = env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        for name in PROGRAMS {
            let _ = fs::remove_file(dir.join(format!("{name}.old")));
        }
    }
}

/// Downloads, verifies, unpacks and installs `release`. Returns the new app to start.
fn install(release: &Release, report: &dyn Fn(Install)) -> Result<PathBuf, String> {
    let (package, checksum) =
        release.package.as_ref().ok_or_else(|| format!("Release {} has no package for this system.", release.version))?;
    let data = download(package, &|share| report(Install::Downloading(share.min(1.0))))?;
    report(Install::Verifying);
    let checksum_file = download(checksum, &|_| {})?;
    verify(&data, &String::from_utf8_lossy(&checksum_file))?;

    report(Install::Unpacking);
    let dir = env::temp_dir().join(format!("gpu-fanctl-update-{}", release.version));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let archive = dir.join(&package.name);
    fs::write(&archive, &data).map_err(|e| format!("Could not save the download: {e}"))?;
    let folder = unpack(&archive, &dir, &package.name)?;

    let gui = if gpu_fanctl::service::installed_path().is_some() {
        // The new version installs itself, including the service.
        report(Install::Installing);
        crate::elevate::run_elevated(&folder.join(PROGRAMS[0]), "reinstall")?;
        gpu_fanctl::service::install_dir().join(PROGRAMS[1])
    } else {
        let exe = env::current_exe().map_err(|e| e.to_string())?;
        let here = exe.parent().ok_or("Could not find this app's folder.")?;
        for name in PROGRAMS {
            replace(&folder, here, name)?;
        }
        here.join(PROGRAMS[1])
    };
    let _ = fs::remove_dir_all(&dir);
    Ok(gui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some([0, 10, 2]));
        assert_eq!(parse_version("1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.2.0"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"));
    }

    fn github_asset(name: &str) -> GitHubAsset {
        GitHubAsset { name: name.into(), browser_download_url: format!("https://example.invalid/{name}"), size: 1 }
    }

    #[test]
    fn picks_this_platforms_package_and_checksum() {
        let names = [
            "FanConnect-II-Controller-v0.3.0-windows-x64.zip",
            "FanConnect-II-Controller-v0.3.0-windows-x64.zip.sha256",
            "FanConnect-II-Controller-v0.3.0-linux-x64.tar.gz",
            "FanConnect-II-Controller-v0.3.0-linux-x64.tar.gz.sha256",
        ];
        let release = from_github(GitHubRelease {
            tag_name: "v0.3.0".into(),
            html_url: "https://github.com/x/y/releases/tag/v0.3.0".into(),
            body: None,
            assets: names.iter().map(|n| github_asset(n)).collect(),
        });
        assert_eq!(release.version, "0.3.0");
        let (package, checksum) = release.package.unwrap();
        assert!(package.name.ends_with(PACKAGE_SUFFIX));
        assert_eq!(checksum.name, format!("{}.sha256", package.name));
    }

    #[test]
    fn release_without_checksum_has_no_package() {
        let release = from_github(GitHubRelease {
            tag_name: "v0.3.0".into(),
            html_url: String::new(),
            body: None,
            assets: vec![github_asset(&format!("FanConnect-II-Controller-v0.3.0{PACKAGE_SUFFIX}"))],
        });
        assert!(!release.has_package());
    }

    /// Everything `install` does except installing: `cargo test -- --ignored`.
    #[test]
    #[ignore = "downloads the latest release from GitHub"]
    fn latest_release_downloads_verifies_and_unpacks() {
        let release = latest_release().unwrap();
        let (package, checksum) = release.package.clone().expect("the latest release has a package for this system");
        let data = download(&package, &|_| {}).unwrap();
        verify(&data, &String::from_utf8_lossy(&download(&checksum, &|_| {}).unwrap())).unwrap();
        let dir = env::temp_dir().join(format!("gpu-fanctl-update-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(&package.name), &data).unwrap();
        let folder = unpack(&dir.join(&package.name), &dir, &package.name).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(folder.ends_with(package.name.trim_end_matches(".zip").trim_end_matches(".tar.gz")));
    }

    #[test]
    fn checksum_is_verified() {
        let hash = "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e";
        assert_eq!(sha256(b"Hello World"), hash);
        assert!(verify(b"Hello World", &format!("{hash}  package.zip\n")).is_ok());
        assert!(verify(b"Hello World", &format!("{}  package.zip\n", hash.to_uppercase())).is_ok());
        assert!(verify(b"Hello world", &format!("{hash}  package.zip")).is_err());
        assert!(verify(b"Hello World", "garbage").is_err());
    }
}
