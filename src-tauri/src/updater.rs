//! Live GitHub Releases update checker and safe external link launcher.

use crate::ipc::{
    detect_os, format_unix_iso8601, unix_timestamp_secs, UpdateCheckResultDto, UpdateStatusDto,
    EVENT_UPDATE_STATUS,
};
use serde::Deserialize;
use std::cmp::Ordering;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_updater::UpdaterExt;

pub const GITHUB_REPO_OWNER: &str = "emrisca";
pub const GITHUB_REPO_NAME: &str = "RatPoison";
pub const GITHUB_RELEASES_PAGE_URL: &str = "https://github.com/emrisca/RatPoison/releases";
pub const STABLE_UPDATER_MANIFEST_URL: &str =
    "https://github.com/emrisca/RatPoison/releases/latest/download/latest.json";
const GITHUB_RELEASES_API_URL: &str =
    "https://api.github.com/repos/emrisca/RatPoison/releases?per_page=15";

#[derive(Debug, Clone)]
pub struct UpdateManagerState {
    pub status: UpdateStatusDto,
    pub in_progress: bool,
}

pub struct SharedUpdater(pub Arc<Mutex<UpdateManagerState>>);

impl SharedUpdater {
    pub fn new(auto_install: bool) -> Self {
        Self(Arc::new(Mutex::new(UpdateManagerState {
            status: UpdateStatusDto::idle(env!("CARGO_PKG_VERSION"), auto_install),
            in_progress: false,
        })))
    }

    pub fn current_status(&self) -> UpdateStatusDto {
        self.0
            .lock()
            .map(|g| g.status.clone())
            .unwrap_or_else(|_| UpdateStatusDto::idle(env!("CARGO_PKG_VERSION"), true))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreIdent {
    Numeric(u64),
    AlphaNum(String),
}

impl Ord for PreIdent {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PreIdent::Numeric(a), PreIdent::Numeric(b)) => a.cmp(b),
            (PreIdent::Numeric(_), PreIdent::AlphaNum(_)) => Ordering::Less,
            (PreIdent::AlphaNum(_), PreIdent::Numeric(_)) => Ordering::Greater,
            (PreIdent::AlphaNum(a), PreIdent::AlphaNum(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PreIdent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<PreIdent>,
    pub raw: String,
}

impl SemVer {
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        let stripped = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let without_build = stripped.split('+').next().unwrap_or(stripped);
        let (core, pre_part) = match without_build.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (without_build, None),
        };
        let mut nums = core.split('.');
        let major = nums.next()?.parse::<u64>().ok()?;
        let minor = nums.next()?.parse::<u64>().ok()?;
        let patch = nums.next()?.parse::<u64>().ok()?;
        if nums.next().is_some() {
            return None;
        }
        let prerelease = match pre_part {
            Some(p) if !p.is_empty() => p
                .split('.')
                .map(|part| match part.parse::<u64>() {
                    Ok(n) => PreIdent::Numeric(n),
                    Err(_) => PreIdent::AlphaNum(part.to_string()),
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Self {
            major,
            minor,
            patch,
            prerelease,
            raw: without_build.to_string(),
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.prerelease.cmp(&other.prerelease),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Selects the preferred installer asset for the given OS (`macos`, `windows`, `linux`).
pub fn select_os_asset<'a>(assets: &'a [GitHubAsset], os: &str) -> Option<&'a GitHubAsset> {
    let find_first = |pred: &dyn Fn(&str) -> bool| -> Option<&'a GitHubAsset> {
        assets.iter().find(|a| pred(&a.name.to_ascii_lowercase()))
    };

    match os {
        "macos" => find_first(&|n| n.ends_with(".dmg") && n.contains("universal"))
            .or_else(|| find_first(&|n| n.ends_with(".dmg")))
            .or_else(|| find_first(&|n| n.ends_with(".app.tar.gz") && n.contains("universal")))
            .or_else(|| find_first(&|n| n.ends_with(".app.tar.gz"))),
        "windows" => find_first(&|n| n.ends_with("-setup.exe"))
            .or_else(|| find_first(&|n| n.ends_with(".exe")))
            .or_else(|| find_first(&|n| n.ends_with(".msi"))),
        _ => find_first(&|n| n.ends_with(".appimage"))
            .or_else(|| find_first(&|n| n.ends_with(".deb")))
            .or_else(|| find_first(&|n| n.ends_with(".rpm"))),
    }
}

/// Evaluates a list of GitHub releases against the running version, channel, and host OS.
pub fn resolve_release_from_list(
    releases: &[GitHubRelease],
    current_version: &str,
    channel_str: &str,
    os: &str,
    is_flatpak: bool,
) -> UpdateCheckResultDto {
    let channel = if channel_str.eq_ignore_ascii_case("beta") {
        "beta"
    } else {
        "stable"
    };
    let current_ver = SemVer::parse(current_version);
    let now_iso = format_unix_iso8601(unix_timestamp_secs());

    let best = releases
        .iter()
        .filter(|rel| !rel.draft)
        .filter_map(|rel| {
            let ver = SemVer::parse(&rel.tag_name)?;
            if channel == "stable" && (rel.prerelease || ver.is_prerelease()) {
                return None;
            }
            Some((ver, rel))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b));

    let Some((best_ver, rel)) = best else {
        let release_notes = if is_flatpak {
            format!(
                "RatPoison v{current_version} is managed via Flatpak (`flatpak update io.github.emrisca.RatPoison`). No newer {channel} releases found on GitHub yet."
            )
        } else {
            format!(
                "No published {channel} releases were found on GitHub yet. You are running RatPoison v{current_version}."
            )
        };
        return UpdateCheckResultDto {
            current_version: current_version.to_string(),
            latest_version: current_version.to_string(),
            update_available: false,
            release_notes,
            published_at: now_iso,
            download_url: Some(GITHUB_RELEASES_PAGE_URL.to_string()),
            release_url: Some(GITHUB_RELEASES_PAGE_URL.to_string()),
            asset_name: None,
        };
    };

    let update_available = match &current_ver {
        Some(cur) => best_ver > *cur,
        None => best_ver.raw != current_version,
    };

    let release_url = if rel.html_url.trim().is_empty() {
        GITHUB_RELEASES_PAGE_URL.to_string()
    } else {
        rel.html_url.trim().to_string()
    };

    let matched_asset = select_os_asset(&rel.assets, os);
    let download_url = matched_asset
        .map(|a| a.browser_download_url.clone())
        .or_else(|| Some(release_url.clone()));
    let asset_name = matched_asset.map(|a| a.name.clone());

    let body_trimmed = rel
        .body
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let release_notes = if is_flatpak {
        match body_trimmed {
            Some(notes) => format!(
                "Flatpak build detected — run `flatpak update io.github.emrisca.RatPoison` to upgrade.\n\n{notes}"
            ),
            None => format!(
                "RatPoison v{} ({channel} channel) is live on GitHub Releases. Update via `flatpak update io.github.emrisca.RatPoison`.",
                best_ver.raw
            ),
        }
    } else {
        match body_trimmed {
            Some(notes) => notes.to_string(),
            None => {
                let title = rel
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&rel.tag_name);
                format!("{title} is published on GitHub Releases ({channel} channel).")
            }
        }
    };

    let published_at = rel
        .published_at
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or(now_iso);

    UpdateCheckResultDto {
        current_version: current_version.to_string(),
        latest_version: best_ver.raw,
        update_available,
        release_notes,
        published_at,
        download_url,
        release_url: Some(release_url),
        asset_name,
    }
}

/// Queries live GitHub Releases for `emrisca/RatPoison`.
pub fn check_github_releases(channel_str: &str) -> Result<UpdateCheckResultDto, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let os = detect_os();
    let is_flatpak = std::env::var_os("FLATPAK_ID").is_some()
        || std::path::Path::new("/.flatpak-info").exists();

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();

    let user_agent = format!("RatPoison/{current_version} (+https://github.com/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME})");

    let response = agent
        .get(GITHUB_RELEASES_API_URL)
        .set("User-Agent", &user_agent)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call();

    match response {
        Ok(resp) => {
            let releases: Vec<GitHubRelease> = resp
                .into_json()
                .map_err(|err| format!("Failed to parse GitHub Releases response: {err}"))?;
            Ok(resolve_release_from_list(
                &releases,
                current_version,
                channel_str,
                os,
                is_flatpak,
            ))
        }
        Err(ureq::Error::Status(404, _)) => Ok(resolve_release_from_list(
            &[],
            current_version,
            channel_str,
            os,
            is_flatpak,
        )),
        Err(ureq::Error::Status(403, _)) => Err(
            "GitHub API rate limit reached. Open GitHub Releases directly to check for new builds."
                .to_string(),
        ),
        Err(ureq::Error::Status(code, _)) => {
            Err(format!("GitHub Releases returned HTTP {code}."))
        }
        Err(ureq::Error::Transport(err)) => {
            Err(format!("Unable to reach GitHub Releases: {err}"))
        }
    }
}

/// Validates that an external URL points strictly to RatPoison's GitHub repo or GitHub release assets.
pub fn validate_external_release_url(url: &str) -> Result<&str, String> {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
    {
        return Err("Invalid release URL.".to_string());
    }

    let repo_root = format!("https://github.com/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME}");
    let repo_prefix = format!("{repo_root}/");

    let allowed = trimmed == repo_root
        || trimmed.starts_with(&repo_prefix)
        || trimmed.starts_with("https://objects.githubusercontent.com/")
        || trimmed.starts_with("https://release-assets.githubusercontent.com/");

    if !allowed {
        return Err(
            "Only official RatPoison GitHub release URLs can be opened from the updater."
                .to_string(),
        );
    }

    Ok(trimmed)
}

/// Opens a validated RatPoison GitHub Releases or installer URL in the user's default browser.
pub fn open_external_url(url: &str) -> Result<(), String> {
    let valid = validate_external_release_url(url)?;

    #[cfg(target_os = "macos")]
    let spawn_result = Command::new("open").arg(valid).spawn();

    #[cfg(target_os = "windows")]
    let spawn_result = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", valid])
        .spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let spawn_result = Command::new("xdg-open").arg(valid).spawn();

    spawn_result
        .map(|_| ())
        .map_err(|err| format!("Failed to open browser for {valid}: {err}"))
}

/// Selects the signed Tauri updater manifest (`latest.json`) URL for the chosen channel.
pub fn select_updater_manifest_url_from_list(releases: &[GitHubRelease], channel_str: &str) -> String {
    let channel = if channel_str.eq_ignore_ascii_case("beta") {
        "beta"
    } else {
        "stable"
    };

    if channel == "stable" {
        return STABLE_UPDATER_MANIFEST_URL.to_string();
    }

    releases
        .iter()
        .filter(|rel| !rel.draft)
        .filter_map(|rel| {
            let ver = SemVer::parse(&rel.tag_name)?;
            let manifest = rel
                .assets
                .iter()
                .find(|a| a.name.eq_ignore_ascii_case("latest.json"))?;
            Some((ver, manifest.browser_download_url.clone()))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, url)| url)
        .unwrap_or_else(|| STABLE_UPDATER_MANIFEST_URL.to_string())
}

/// Resolves the active `latest.json` endpoint URL for `stable` or `beta`.
pub fn resolve_updater_manifest_url(channel_str: &str) -> String {
    if !channel_str.eq_ignore_ascii_case("beta") {
        return STABLE_UPDATER_MANIFEST_URL.to_string();
    }

    let current_version = env!("CARGO_PKG_VERSION");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build();
    let user_agent = format!(
        "RatPoison/{current_version} (+https://github.com/{GITHUB_REPO_OWNER}/{GITHUB_REPO_NAME})"
    );

    match agent
        .get(GITHUB_RELEASES_API_URL)
        .set("User-Agent", &user_agent)
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(resp) => match resp.into_json::<Vec<GitHubRelease>>() {
            Ok(releases) => select_updater_manifest_url_from_list(&releases, "beta"),
            Err(_) => STABLE_UPDATER_MANIFEST_URL.to_string(),
        },
        Err(_) => STABLE_UPDATER_MANIFEST_URL.to_string(),
    }
}

fn set_and_emit_status(app: &AppHandle, status: UpdateStatusDto) {
    if let Some(shared) = app.try_state::<SharedUpdater>() {
        if let Ok(mut guard) = shared.0.lock() {
            guard.status = status.clone();
        }
    }
    let _ = app.emit(EVENT_UPDATE_STATUS, &status);
}

fn finish_in_progress(app: &AppHandle) {
    if let Some(shared) = app.try_state::<SharedUpdater>() {
        if let Ok(mut guard) = shared.0.lock() {
            guard.in_progress = false;
        }
    }
}

/// Checks the signed Tauri updater manifest and optionally downloads, verifies, installs
/// in-place, and restarts RatPoison automatically.
pub async fn run_in_app_update(
    app: AppHandle,
    channel: String,
    install_now: bool,
    auto_install: bool,
) -> Result<UpdateStatusDto, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let is_flatpak = std::env::var_os("FLATPAK_ID").is_some()
        || std::path::Path::new("/.flatpak-info").exists();

    if is_flatpak {
        let status = UpdateStatusDto {
            state: "upToDate".to_string(),
            current_version,
            latest_version: None,
            downloaded_bytes: 0,
            total_bytes: None,
            percent: None,
            release_notes: None,
            published_at: None,
            message: Some(
                "Flatpak builds update via `flatpak update io.github.emrisca.RatPoison`."
                    .to_string(),
            ),
            auto_install,
        };
        set_and_emit_status(&app, status.clone());
        return Ok(status);
    }

    // Prevent concurrent checks/installs from colliding.
    if let Some(shared) = app.try_state::<SharedUpdater>() {
        let mut guard = shared
            .0
            .lock()
            .map_err(|_| "updater state lock poisoned".to_string())?;
        if guard.in_progress {
            return Ok(guard.status.clone());
        }
        guard.in_progress = true;
    }

    let checking_status = UpdateStatusDto {
        state: "checking".to_string(),
        current_version: current_version.clone(),
        latest_version: None,
        downloaded_bytes: 0,
        total_bytes: None,
        percent: None,
        release_notes: None,
        published_at: None,
        message: Some("Checking for signed updates…".to_string()),
        auto_install,
    };
    set_and_emit_status(&app, checking_status);

    let channel_for_url = channel.clone();
    let manifest_url_str = tauri::async_runtime::spawn_blocking(move || {
        resolve_updater_manifest_url(&channel_for_url)
    })
    .await
    .unwrap_or_else(|_| STABLE_UPDATER_MANIFEST_URL.to_string());

    let endpoint_url = match Url::parse(&manifest_url_str) {
        Ok(u) => u,
        Err(err) => {
            let msg = format!("Invalid updater endpoint URL: {err}");
            let status = UpdateStatusDto {
                state: "error".to_string(),
                current_version,
                latest_version: None,
                downloaded_bytes: 0,
                total_bytes: None,
                percent: None,
                release_notes: None,
                published_at: None,
                message: Some(msg.clone()),
                auto_install,
            };
            set_and_emit_status(&app, status);
            finish_in_progress(&app);
            return Err(msg);
        }
    };

    let updater = match app
        .updater_builder()
        .endpoints(vec![endpoint_url])
        .and_then(|b| b.build())
    {
        Ok(u) => u,
        Err(err) => {
            let msg = format!("Failed to initialize in-app updater: {err}");
            let status = UpdateStatusDto {
                state: "error".to_string(),
                current_version,
                latest_version: None,
                downloaded_bytes: 0,
                total_bytes: None,
                percent: None,
                release_notes: None,
                published_at: None,
                message: Some(msg.clone()),
                auto_install,
            };
            set_and_emit_status(&app, status);
            finish_in_progress(&app);
            return Err(msg);
        }
    };

    let maybe_update = match updater.check().await {
        Ok(opt) => opt,
        Err(err) => {
            let msg = format!("Could not check for updates: {err}");
            let status = UpdateStatusDto {
                state: "error".to_string(),
                current_version,
                latest_version: None,
                downloaded_bytes: 0,
                total_bytes: None,
                percent: None,
                release_notes: None,
                published_at: None,
                message: Some(msg.clone()),
                auto_install,
            };
            set_and_emit_status(&app, status);
            finish_in_progress(&app);
            return Err(msg);
        }
    };

    let Some(update) = maybe_update else {
        let status = UpdateStatusDto {
            state: "upToDate".to_string(),
            current_version: current_version.clone(),
            latest_version: Some(current_version),
            downloaded_bytes: 0,
            total_bytes: None,
            percent: None,
            release_notes: None,
            published_at: None,
            message: Some("RatPoison is up to date.".to_string()),
            auto_install,
        };
        set_and_emit_status(&app, status.clone());
        finish_in_progress(&app);
        return Ok(status);
    };

    let latest_version = update.version.clone();
    let release_notes = update.body.clone();
    let published_at = update.date.map(|d| d.to_string());

    if !install_now {
        let status = UpdateStatusDto {
            state: "available".to_string(),
            current_version,
            latest_version: Some(latest_version.clone()),
            downloaded_bytes: 0,
            total_bytes: None,
            percent: None,
            release_notes,
            published_at,
            message: Some(format!("RatPoison v{latest_version} is ready to install.")),
            auto_install,
        };
        set_and_emit_status(&app, status.clone());
        finish_in_progress(&app);
        return Ok(status);
    }

    let downloading_status = UpdateStatusDto {
        state: "downloading".to_string(),
        current_version: current_version.clone(),
        latest_version: Some(latest_version.clone()),
        downloaded_bytes: 0,
        total_bytes: None,
        percent: Some(0),
        release_notes: release_notes.clone(),
        published_at: published_at.clone(),
        message: Some(format!("Downloading RatPoison v{latest_version}…")),
        auto_install,
    };
    set_and_emit_status(&app, downloading_status);

    let app_for_progress = app.clone();
    let cur_for_progress = current_version.clone();
    let latest_for_progress = latest_version.clone();
    let notes_for_progress = release_notes.clone();
    let pub_for_progress = published_at.clone();

    let app_for_finish = app.clone();
    let cur_for_finish = current_version.clone();
    let latest_for_finish = latest_version.clone();
    let notes_for_finish = release_notes.clone();
    let pub_for_finish = published_at.clone();

    let mut downloaded_total: u64 = 0;
    let mut last_emitted_percent: Option<u8> = Some(0);
    let mut last_emitted_at = Instant::now();

    let install_result = update
        .download_and_install(
            move |chunk_length, content_length| {
                downloaded_total = downloaded_total.saturating_add(chunk_length as u64);
                let pct = content_length
                    .filter(|&total| total > 0)
                    .map(|total| ((downloaded_total.saturating_mul(100)) / total).min(100) as u8);

                let should_emit = last_emitted_at.elapsed() >= Duration::from_millis(120)
                    || pct.is_some_and(|p| p >= 100 || last_emitted_percent.is_none_or(|prev| p >= prev.saturating_add(2)));

                if should_emit {
                    last_emitted_at = Instant::now();
                    last_emitted_percent = pct;
                    let status = UpdateStatusDto {
                        state: "downloading".to_string(),
                        current_version: cur_for_progress.clone(),
                        latest_version: Some(latest_for_progress.clone()),
                        downloaded_bytes: downloaded_total,
                        total_bytes: content_length,
                        percent: pct,
                        release_notes: notes_for_progress.clone(),
                        published_at: pub_for_progress.clone(),
                        message: Some(match pct {
                            Some(p) => format!("Downloading RatPoison v{latest_for_progress} ({p}%)…"),
                            None => format!("Downloading RatPoison v{latest_for_progress}…"),
                        }),
                        auto_install,
                    };
                    set_and_emit_status(&app_for_progress, status);
                }
            },
            move || {
                let status = UpdateStatusDto {
                    state: "installing".to_string(),
                    current_version: cur_for_finish.clone(),
                    latest_version: Some(latest_for_finish.clone()),
                    downloaded_bytes: 0,
                    total_bytes: None,
                    percent: Some(100),
                    release_notes: notes_for_finish.clone(),
                    published_at: pub_for_finish.clone(),
                    message: Some(format!(
                        "Installing RatPoison v{latest_for_finish} and restarting…"
                    )),
                    auto_install,
                };
                set_and_emit_status(&app_for_finish, status);
            },
        )
        .await;

    if let Err(err) = install_result {
        let msg = format!("Failed to install update v{latest_version}: {err}");
        let status = UpdateStatusDto {
            state: "error".to_string(),
            current_version,
            latest_version: Some(latest_version),
            downloaded_bytes: 0,
            total_bytes: None,
            percent: None,
            release_notes,
            published_at,
            message: Some(msg.clone()),
            auto_install,
        };
        set_and_emit_status(&app, status);
        finish_in_progress(&app);
        return Err(msg);
    }

    let ready_status = UpdateStatusDto {
        state: "ready".to_string(),
        current_version,
        latest_version: Some(latest_version.clone()),
        downloaded_bytes: 0,
        total_bytes: None,
        percent: Some(100),
        release_notes,
        published_at,
        message: Some(format!(
            "RatPoison v{latest_version} installed. Restarting…"
        )),
        auto_install,
    };
    set_and_emit_status(&app, ready_status.clone());
    finish_in_progress(&app);

    // Give the UI a brief moment to paint the restarting state, then relaunch into the new build.
    let app_for_restart = app.clone();
    tauri::async_runtime::spawn(async move {
        std::thread::sleep(Duration::from_millis(450));
        app_for_restart.restart();
    });

    Ok(ready_status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_assets() -> Vec<GitHubAsset> {
        vec![
            GitHubAsset {
                name: "RatPoison_0.2.0_universal.dmg".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_0.2.0_universal.dmg"
                        .to_string(),
            },
            GitHubAsset {
                name: "RatPoison_universal.app.tar.gz".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_universal.app.tar.gz"
                        .to_string(),
            },
            GitHubAsset {
                name: "RatPoison_0.2.0_x64-setup.exe".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_0.2.0_x64-setup.exe"
                        .to_string(),
            },
            GitHubAsset {
                name: "RatPoison_0.2.0_x64_en-US.msi".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_0.2.0_x64_en-US.msi"
                        .to_string(),
            },
            GitHubAsset {
                name: "RatPoison_0.2.0_amd64.deb".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_0.2.0_amd64.deb"
                        .to_string(),
            },
            GitHubAsset {
                name: "RatPoison_0.2.0_amd64.AppImage".to_string(),
                browser_download_url:
                    "https://github.com/emrisca/RatPoison/releases/download/v0.2.0/RatPoison_0.2.0_amd64.AppImage"
                        .to_string(),
            },
        ]
    }

    fn make_release(
        tag: &str,
        draft: bool,
        prerelease: bool,
        body: &str,
        assets: Vec<GitHubAsset>,
    ) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.to_string(),
            name: Some(format!("RatPoison {tag}")),
            body: Some(body.to_string()),
            draft,
            prerelease,
            html_url: format!("https://github.com/emrisca/RatPoison/releases/tag/{tag}"),
            published_at: Some("2026-11-23T01:00:00Z".to_string()),
            assets,
        }
    }

    #[test]
    fn semver_ordering_handles_prereleases_and_v_prefix() {
        let v010 = SemVer::parse("v0.1.0").expect("v0.1.0");
        let v020_alpha = SemVer::parse("0.2.0-alpha.1").expect("alpha");
        let v020_beta1 = SemVer::parse("v0.2.0-beta.1").expect("beta.1");
        let v020_beta2 = SemVer::parse("v0.2.0-beta.2").expect("beta.2");
        let v020_beta10 = SemVer::parse("v0.2.0-beta.10").expect("beta.10");
        let v020 = SemVer::parse("v0.2.0").expect("v0.2.0");

        assert!(v010 < v020_alpha);
        assert!(v020_alpha < v020_beta1);
        assert!(v020_beta1 < v020_beta2);
        assert!(v020_beta2 < v020_beta10);
        assert!(v020_beta10 < v020);
        assert!(SemVer::parse("not-a-version").is_none());
    }

    #[test]
    fn stable_channel_skips_drafts_and_prereleases_and_selects_highest() {
        let releases = vec![
            make_release("v0.3.0", true, false, "draft release", sample_assets()),
            make_release("v0.2.0-beta.1", false, true, "beta notes", sample_assets()),
            make_release("v0.1.0", false, false, "initial stable", Vec::new()),
            make_release("v0.2.0", false, false, "stable 0.2.0 notes", sample_assets()),
        ];

        let res = resolve_release_from_list(&releases, "0.1.0", "stable", "macos", false);
        assert_eq!(res.current_version, "0.1.0");
        assert_eq!(res.latest_version, "0.2.0");
        assert!(res.update_available);
        assert_eq!(res.release_notes, "stable 0.2.0 notes");
        assert_eq!(
            res.asset_name.as_deref(),
            Some("RatPoison_0.2.0_universal.dmg")
        );
        assert_eq!(
            res.release_url.as_deref(),
            Some("https://github.com/emrisca/RatPoison/releases/tag/v0.2.0")
        );
    }

    #[test]
    fn beta_channel_includes_prereleases_when_newer() {
        let releases = vec![
            make_release("v0.2.0-beta.1", false, true, "beta preview", sample_assets()),
            make_release("v0.1.0", false, false, "stable 0.1.0", Vec::new()),
        ];

        let stable = resolve_release_from_list(&releases, "0.1.0", "stable", "macos", false);
        assert!(!stable.update_available);
        assert_eq!(stable.latest_version, "0.1.0");

        let beta = resolve_release_from_list(&releases, "0.1.0", "beta", "macos", false);
        assert!(beta.update_available);
        assert_eq!(beta.latest_version, "0.2.0-beta.1");
        assert_eq!(beta.release_notes, "beta preview");
    }

    #[test]
    fn os_asset_matching_prefers_native_installers() {
        let assets = sample_assets();
        assert_eq!(
            select_os_asset(&assets, "macos").map(|a| a.name.as_str()),
            Some("RatPoison_0.2.0_universal.dmg")
        );
        assert_eq!(
            select_os_asset(&assets, "windows").map(|a| a.name.as_str()),
            Some("RatPoison_0.2.0_x64-setup.exe")
        );
        assert_eq!(
            select_os_asset(&assets, "linux").map(|a| a.name.as_str()),
            Some("RatPoison_0.2.0_amd64.AppImage")
        );

        // Falls back cleanly to release html_url when no installer matches.
        let no_asset_release = vec![make_release("v0.2.0", false, false, "notes", Vec::new())];
        let res = resolve_release_from_list(&no_asset_release, "0.1.0", "linux", "linux", false);
        assert!(res.asset_name.is_none());
        assert_eq!(
            res.download_url.as_deref(),
            Some("https://github.com/emrisca/RatPoison/releases/tag/v0.2.0")
        );
    }

    #[test]
    fn empty_release_list_reports_up_to_date_with_releases_link() {
        let res = resolve_release_from_list(&[], "0.1.0", "stable", "macos", false);
        assert!(!res.update_available);
        assert_eq!(res.latest_version, "0.1.0");
        assert_eq!(res.release_url.as_deref(), Some(GITHUB_RELEASES_PAGE_URL));
        assert_eq!(res.download_url.as_deref(), Some(GITHUB_RELEASES_PAGE_URL));
    }

    #[test]
    fn external_url_allowlist_accepts_repo_and_rejects_arbitrary_urls() {
        assert!(validate_external_release_url(GITHUB_RELEASES_PAGE_URL).is_ok());
        assert!(validate_external_release_url(
            "https://github.com/emrisca/RatPoison/releases/download/v0.1.0/RatPoison_0.1.0_universal.dmg"
        )
        .is_ok());
        assert!(validate_external_release_url(
            "https://objects.githubusercontent.com/github-production-release-asset-123"
        )
        .is_ok());

        assert!(validate_external_release_url("https://evil.example.com/payload.dmg").is_err());
        assert!(validate_external_release_url("https://github.com/emrisca/RatPoisonEvil/releases").is_err());
        assert!(validate_external_release_url("file:///etc/passwd").is_err());
        assert!(validate_external_release_url("https://github.com/emrisca/RatPoison/releases\nopen /bin/sh").is_err());
    }

    #[test]
    fn updater_manifest_url_resolves_stable_and_beta_channels() {
        let stable_url = select_updater_manifest_url_from_list(&[], "stable");
        assert_eq!(stable_url, STABLE_UPDATER_MANIFEST_URL);

        let releases = vec![
            make_release(
                "v0.3.0-beta.2",
                true,
                true,
                "draft",
                vec![GitHubAsset {
                    name: "latest.json".to_string(),
                    browser_download_url:
                        "https://github.com/emrisca/RatPoison/releases/download/v0.3.0-beta.2/latest.json"
                            .to_string(),
                }],
            ),
            make_release(
                "v0.2.0-beta.1",
                false,
                true,
                "beta",
                vec![GitHubAsset {
                    name: "latest.json".to_string(),
                    browser_download_url:
                        "https://github.com/emrisca/RatPoison/releases/download/v0.2.0-beta.1/latest.json"
                            .to_string(),
                }],
            ),
            make_release("v0.2.1", false, false, "no manifest", sample_assets()),
        ];

        assert_eq!(
            select_updater_manifest_url_from_list(&releases, "beta"),
            "https://github.com/emrisca/RatPoison/releases/download/v0.2.0-beta.1/latest.json"
        );
    }
}
