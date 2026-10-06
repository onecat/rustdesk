use crate::{common::do_check_software_update, hbbs_http::create_http_client_with_url_strict};
use hbb_common::{bail, config, log, ResultType};
use base::config::keys;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};

#[cfg(target_os = "macos")]
struct MacUpdateLock {
    _file: std::fs::File,
}

#[cfg(target_os = "macos")]
fn acquire_mac_update_lock() -> ResultType<MacUpdateLock> {
    let path = std::path::PathBuf::from("/var/run/rustdesk-update.lock");
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(hbb_common::libc::O_NOFOLLOW | hbb_common::libc::O_CLOEXEC)
        .open(&path)?;
    let metadata = handle.metadata()?;
    if !metadata.file_type().is_file() || metadata.uid() != 0 {
        bail!("[root-update] update lock is not a root-owned regular file");
    }
    handle.set_permissions(std::fs::Permissions::from_mode(0o600))?;

    // Keep the descriptor open through update preparation and detached-script
    // launch. O_CLOEXEC means this lock does not cover the detached bundle
    // swap; flock is released when this guard is dropped or the process exits.
    let lock_result = unsafe {
        hbb_common::libc::flock(
            handle.as_raw_fd(),
            hbb_common::libc::LOCK_EX | hbb_common::libc::LOCK_NB,
        )
    };
    if lock_result != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            bail!("[root-update] another update is already running");
        }
        return Err(err.into());
    }
    Ok(MacUpdateLock { _file: handle })
}

enum UpdateMsg {
    CheckUpdate,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    Done,
    RetrySoon,
}

lazy_static::lazy_static! {
    static ref TX_MSG : Mutex<Sender<UpdateMsg>> = Mutex::new(start_auto_update_check());
}

static CONTROLLING_SESSION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Initial wait after startup before the first update check (30 seconds).
pub const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(30);

/// One full day — default interval between update checks.
pub const DUR_ONE_DAY: Duration = Duration::from_secs(60 * 60 * 24);

/// Minimum interval between consecutive update checks (10 minutes).
pub const MIN_INTERVAL: Duration = Duration::from_secs(60 * 10);

/// Retry interval when an update check fails or a session is active (30 minutes).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 30);

const MANAGED_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60 * 6);
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 30);
const MIN_INTERVAL: Duration = Duration::from_secs(60 * 10);
const MANAGED_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MANAGED_MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
const MANAGED_PACKAGE_TIMEOUT: Duration = Duration::from_secs(60 * 10);
const MANAGED_MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MANAGED_REPO_RELEASE_PREFIX: &str =
    "https://github.com/onecat/rustdesk/releases/download/";

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize)]
struct ManagedManifestPayload {
    schema: u32,
    channel: String,
    version: String,
    build: u64,
    published_at: String,
    package: ManagedPackage,
    rollout: u8,
    mandatory: bool,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize)]
struct ManagedPackage {
    url: String,
    sha256: String,
    size: u64,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Serialize)]
struct ManagedUpdateState<'a> {
    current_version: &'a str,
    current_build: u64,
    channel: &'a str,
    last_check_unix: u64,
    available_version: Option<&'a str>,
    available_build: Option<u64>,
    downloaded: bool,
    source: Option<&'a str>,
    last_result: &'a str,
    last_error: Option<&'a str>,
}

pub fn update_controlling_session_count(count: usize) {
    CONTROLLING_SESSION_COUNT.store(count, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn start_auto_update() {
    let _sender = TX_MSG.lock().unwrap();
}

#[allow(dead_code)]
pub fn manually_check_update() -> ResultType<()> {
    let sender = TX_MSG.lock().unwrap();
    sender.send(UpdateMsg::CheckUpdate)?;
    Ok(())
}

#[allow(dead_code)]
pub fn stop_auto_update() {
    let sender = TX_MSG.lock().unwrap();
    sender.send(UpdateMsg::Exit).unwrap_or_default();
}

#[inline]
/// Returns true when there are no active incoming or outgoing connections.
/// Used to avoid updating while a remote session is in progress.
pub fn has_no_active_conns() -> bool {
    let conns = crate::Connection::alive_conns();
    conns.is_empty() && has_no_controlling_conns()
}

#[cfg(any(not(target_os = "windows"), feature = "flutter"))]
fn has_no_controlling_conns() -> bool {
    CONTROLLING_SESSION_COUNT.load(Ordering::SeqCst) == 0
}

#[cfg(not(any(not(target_os = "windows"), feature = "flutter")))]
fn has_no_controlling_conns() -> bool {
    let app_exe = format!("{}.exe", crate::get_app_name().to_lowercase());
    for arg in [
        "--connect",
        "--play",
        "--file-transfer",
        "--view-camera",
        "--port-forward",
        "--rdp",
    ] {
        if !crate::platform::get_pids_of_process_with_first_arg(&app_exe, arg).is_empty() {
            return false;
        }
    }
    true
}

fn start_auto_update_check() -> Sender<UpdateMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || start_auto_update_check_(rx));
    tx
}

fn start_auto_update_check_(rx_msg: Receiver<UpdateMsg>) {
    let initial_delay = if crate::managed_config::managed_updates_enabled() {
        120 + (hbb_common::rand::random::<u64>() % 481)
    } else {
        INITIAL_CHECK_DELAY.as_secs()
    };
    std::thread::sleep(Duration::from_secs(initial_delay));

    let mut check_interval = normal_check_interval();
    let mut last_check_time = Instant::now()
        .checked_sub(MIN_INTERVAL)
        .unwrap_or_else(Instant::now);

    match check_update(false) {
        Ok(CheckOutcome::RetrySoon) => check_interval = RETRY_INTERVAL,
        Ok(CheckOutcome::Done) => {
            last_check_time = Instant::now();
            check_interval = normal_check_interval();
        }
        Err(e) => {
            let error = e.to_string();
            log::error!("Error checking for updates: {}", error);
            #[cfg(target_os = "windows")]
            write_managed_state_with_error(None, false, None, "check-failed", Some(&error));
            check_interval = RETRY_INTERVAL;
        }
    }

    loop {
        let recv_res = rx_msg.recv_timeout(check_interval);
        match &recv_res {
            Ok(UpdateMsg::CheckUpdate) | Err(_) => {
                if last_check_time.elapsed() < MIN_INTERVAL {
                    continue;
                }
                if !crate::managed_config::managed_updates_enabled() && !has_no_active_conns() {
                    check_interval = RETRY_INTERVAL;
                    continue;
                }
                match check_update(matches!(&recv_res, Ok(UpdateMsg::CheckUpdate))) {
                    Ok(CheckOutcome::Done) => {
                        last_check_time = Instant::now();
                        check_interval = normal_check_interval();
                    }
                    Ok(CheckOutcome::RetrySoon) => {
                        last_check_time = Instant::now();
                        check_interval = RETRY_INTERVAL;
                    }
                    Err(e) => {
                        let error = e.to_string();
                        log::error!("Error checking for updates: {}", error);
                        #[cfg(target_os = "windows")]
                        write_managed_state_with_error(None, false, None, "check-failed", Some(&error));
                        check_interval = RETRY_INTERVAL;
                    }
                }
            }
            Ok(UpdateMsg::Exit) => break,
        }
    }
}

fn normal_check_interval() -> Duration {
    if crate::managed_config::managed_updates_enabled() {
        MANAGED_CHECK_INTERVAL
    } else {
        DUR_ONE_DAY
    }
}

fn check_update(manually: bool) -> ResultType<CheckOutcome> {
    #[cfg(target_os = "windows")]
    if crate::managed_config::managed_updates_enabled() && crate::platform::is_msi_installed()? {
        return check_managed_update();
    }

    check_legacy_update(manually)?;
    Ok(CheckOutcome::Done)
}

fn check_legacy_update(manually: bool) -> ResultType<()> {
    // On macOS, auto-update is handled by check_update_as_root() in the service process.
    // The shared check_update() path is only used for manual update checks from the GUI.
    #[cfg(target_os = "macos")]
    if !manually {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    let update_msi = crate::platform::is_msi_installed()? && !crate::is_custom_client();
    if !(manually || config::Config::get_bool_option(keys::OPTION_ALLOW_AUTO_UPDATE)) {
        return Ok(());
    }
    if do_check_software_update().is_err() {
        // ignore
        return Ok(());
    }

    let update_url = crate::common::SOFTWARE_UPDATE_URL.lock().unwrap().clone();
    if update_url.is_empty() {
        log::debug!("No update available.");
    } else {
        let download_url = update_url.replace("tag", "download");
        let version = download_url.split('/').last().unwrap_or_default();
        #[cfg(target_os = "windows")]
        let download_url = if cfg!(feature = "flutter") {
            let Some(arch) = crate::platform::windows::release_arch_suffix() else {
                bail!(
                    "Unsupported Windows release architecture: {}",
                    std::env::consts::ARCH
                );
            };
            format!(
                "{}/rustdesk-{}-{}.{}",
                download_url,
                version,
                arch,
                if update_msi { "msi" } else { "exe" }
            )
        } else {
            format!("{}/rustdesk-{}-x86-sciter.exe", download_url, version)
        };
        log::debug!("New version available: {}", &version);
        let client = create_http_client_with_url_strict(&download_url)?;
        let Some(file_path) = get_download_file_from_url(&download_url) else {
            bail!("Failed to get the file path from the URL: {}", download_url);
        };
        let mut is_file_exists = false;
        if file_path.exists() {
            // Check if the file size is the same as the server file size
            // If the file size is the same, we don't need to download it again.
            let file_size = std::fs::metadata(&file_path)?.len();
            let response = client.head(&download_url).send()?;
            if !response.status().is_success() {
                bail!("Failed to get the file size: {}", response.status());
            }
            let total_size = response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|ct_len| ct_len.to_str().ok())
                .and_then(|ct_len| ct_len.parse::<u64>().ok());
            let Some(total_size) = total_size else {
                bail!("Failed to get content length");
            };
            if file_size == total_size {
                is_file_exists = true;
            } else {
                std::fs::remove_file(&file_path)?;
            }
        }
        if !is_file_exists {
            let response = client.get(&download_url).send()?;
            if !response.status().is_success() {
                bail!(
                    "Failed to download the new version file: {}",
                    response.status()
                );
            }
            let file_data = response.bytes()?;
            let mut file = std::fs::File::create(&file_path)?;
            file.write_all(&file_data)?;
        }
        // We have checked if the `conns` is empty before, but we need to check again.
        // No need to care about the downloaded file here, because it's rare case that the `conns` are empty
        // before the download, but not empty after the download.
        if has_no_active_conns() {
            #[cfg(target_os = "windows")]
            update_new_version(update_msi, &version, &file_path);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn check_managed_update() -> ResultType<CheckOutcome> {
    let channel = crate::managed_config::managed_update_channel();
    let manifest_base_url = crate::managed_config::managed_update_manifest_url();
    let cache_bucket = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / MANAGED_CHECK_INTERVAL.as_secs())
        .unwrap_or_default();
    let manifest_url = format!("{}?managed_check={}", manifest_base_url, cache_bucket);

    log::info!(
        "Managed update check: current={} build={} channel={}",
        crate::managed_config::MANAGED_VERSION,
        crate::managed_config::MANAGED_BUILD,
        channel
    );

    let (manifest_text, manifest_used_fallback) = fetch_text_with_fallback(&manifest_url, false)?;
    let payload = parse_managed_manifest(&manifest_text)?;

    validate_managed_manifest(&payload, &channel)?;
    write_managed_state(
        Some(&payload),
        false,
        if manifest_used_fallback {
            Some("fallback")
        } else {
            Some("github")
        },
        "checked",
    );

    if payload.build <= crate::managed_config::MANAGED_BUILD {
        log::debug!(
            "Managed client is up to date: current build {}, available build {}",
            crate::managed_config::MANAGED_BUILD,
            payload.build
        );
        write_managed_state(
            Some(&payload),
            false,
            if manifest_used_fallback {
                Some("fallback")
            } else {
                Some("github")
            },
            "up-to-date",
        );
        return Ok(CheckOutcome::Done);
    }

    if !selected_for_rollout(payload.rollout, payload.build)? {
        log::info!(
            "Managed update {} build {} is outside this device's {}% rollout cohort.",
            payload.version,
            payload.build,
            payload.rollout
        );
        write_managed_state(
            Some(&payload),
            false,
            if manifest_used_fallback {
                Some("fallback")
            } else {
                Some("github")
            },
            "rollout-wait",
        );
        return Ok(CheckOutcome::Done);
    }

    log::info!(
        "Managed update available: {} build {} (published {}, mandatory={})",
        payload.version,
        payload.build,
        payload.published_at,
        payload.mandatory
    );

    let package_path = download_managed_package(&payload, manifest_used_fallback)?;
    write_managed_state(
        Some(&payload),
        true,
        if manifest_used_fallback {
            Some("fallback-preferred")
        } else {
            Some("github-preferred")
        },
        "downloaded",
    );

    if !has_no_active_conns() {
        log::info!(
            "Managed update {} is downloaded; installation deferred because a remote session is active.",
            payload.version
        );
        write_managed_state(
            Some(&payload),
            true,
            if manifest_used_fallback {
                Some("fallback-preferred")
            } else {
                Some("github-preferred")
            },
            "waiting-for-idle",
        );
        return Ok(CheckOutcome::RetrySoon);
    }

    install_managed_package(&payload, &package_path)?;
    Ok(CheckOutcome::Done)
}

#[cfg(target_os = "windows")]
fn strict_update_client(request_timeout: Duration) -> ResultType<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .connect_timeout(MANAGED_CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
}

#[cfg(target_os = "windows")]
fn github_fallback_url(primary: &str) -> Option<String> {
    if !primary.starts_with("https://github.com/") {
        return None;
    }
    Some(format!(
        "{}{}",
        crate::managed_config::MANAGED_GITHUB_FALLBACK_PREFIX,
        primary
    ))
}

#[cfg(target_os = "windows")]
fn ordered_sources(primary: &str, prefer_fallback: bool) -> Vec<(String, bool)> {
    let fallback = github_fallback_url(primary);
    match (fallback, prefer_fallback) {
        (Some(fallback), true) => vec![(fallback, true), (primary.to_owned(), false)],
        (Some(fallback), false) => vec![(primary.to_owned(), false), (fallback, true)],
        (None, _) => vec![(primary.to_owned(), false)],
    }
}

#[cfg(target_os = "windows")]
fn should_try_fallback(status: reqwest::StatusCode) -> bool {
    // A real 404 normally means the release/asset has not been published.
    // Other failures can be caused by regional connectivity, throttling,
    // filtering or an intermediary, so try the alternate source.
    status != reqwest::StatusCode::NOT_FOUND
}

#[cfg(target_os = "windows")]
fn fetch_text_with_fallback(primary: &str, prefer_fallback: bool) -> ResultType<(String, bool)> {
    if !primary.starts_with("https://github.com/") {
        bail!("Managed update URL is not an approved GitHub URL: {}", primary);
    }

    let sources = ordered_sources(primary, prefer_fallback);
    let mut last_error = String::new();
    for (index, (url, is_fallback)) in sources.iter().enumerate() {
        let client = strict_update_client(MANAGED_MANIFEST_TIMEOUT)?;
        match client.get(url).send() {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes()?;
                if bytes.len() > MANAGED_MAX_MANIFEST_BYTES {
                    bail!(
                        "Managed update manifest is too large: {} bytes",
                        bytes.len()
                    );
                }
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| hbb_common::anyhow::anyhow!("Managed update manifest is not UTF-8"))?;
                return Ok((text, *is_fallback));
            }
            Ok(response) => {
                let status = response.status();
                last_error = format!("HTTP {} from {}", status, url);
                if status == reqwest::StatusCode::NOT_FOUND {
                    bail!("{}", last_error);
                }
                let has_more = index + 1 < sources.len();
                if !(has_more && should_try_fallback(status)) {
                    bail!("{}", last_error);
                }
                log::warn!(
                    "Managed update primary request failed ({}); trying alternate source.",
                    status
                );
            }
            Err(e) => {
                last_error = format!("{}: {}", url, e);
                if index + 1 >= sources.len() {
                    break;
                }
                log::warn!(
                    "Managed update request failed for {}; trying alternate source: {}",
                    url,
                    e
                );
            }
        }
    }
    bail!("Managed update request failed: {}", last_error)
}

#[cfg(target_os = "windows")]
fn parse_managed_manifest(body: &str) -> ResultType<ManagedManifestPayload> {
    Ok(serde_json::from_str(body)?)
}

#[cfg(target_os = "windows")]
fn validate_managed_manifest(payload: &ManagedManifestPayload, expected_channel: &str) -> ResultType<()> {
    if payload.schema != 1 {
        bail!("Unsupported managed update schema: {}", payload.schema);
    }
    if payload.channel != expected_channel {
        bail!(
            "Managed update channel mismatch: expected {}, got {}",
            expected_channel,
            payload.channel
        );
    }
    if payload.version.trim().is_empty() {
        bail!("Managed update version is empty");
    }
    if payload.rollout > 100 {
        bail!("Managed update rollout must be in the range 0..=100");
    }
    if payload.package.size == 0 {
        bail!("Managed update package size is zero");
    }
    if payload.package.sha256.len() != 64
        || !payload
            .package
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        bail!("Managed update package SHA-256 is invalid");
    }
    if !payload.package.url.starts_with(MANAGED_REPO_RELEASE_PREFIX)
        || !payload.package.url.to_ascii_lowercase().ends_with(".msi")
    {
        bail!(
            "Managed update package URL is not an approved RustDesk Managed MSI URL: {}",
            payload.package.url
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn managed_update_dir() -> PathBuf {
    let base = std::env::var_os("PROGRAMDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("RustDesk").join("managed-update")
}

#[cfg(target_os = "windows")]
pub fn managed_update_state_json() -> String {
    let path = managed_update_dir().join("state.json");
    fs::read_to_string(path).unwrap_or_else(|_| {
        serde_json::json!({
            "current_version": crate::managed_config::MANAGED_VERSION,
            "current_build": crate::managed_config::MANAGED_BUILD,
            "channel": crate::managed_config::managed_update_channel(),
            "last_check_unix": 0,
            "available_version": serde_json::Value::Null,
            "available_build": serde_json::Value::Null,
            "downloaded": false,
            "source": serde_json::Value::Null,
            "last_result": "not-checked",
            "last_error": serde_json::Value::Null,
        })
        .to_string()
    })
}

#[cfg(not(target_os = "windows"))]
pub fn managed_update_state_json() -> String {
    "{}".to_owned()
}

#[cfg(target_os = "windows")]
fn managed_cohort_id() -> ResultType<String> {
    let dir = managed_update_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join("cohort-id");
    if let Ok(existing) = fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() && existing.len() <= 128 {
            return Ok(existing.to_owned());
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    fs::write(&path, &id)?;
    Ok(id)
}

#[cfg(target_os = "windows")]
fn selected_for_rollout(rollout: u8, build: u64) -> ResultType<bool> {
    if rollout >= 100 {
        return Ok(true);
    }
    if rollout == 0 {
        return Ok(false);
    }

    let cohort = managed_cohort_id()?;
    let mut hasher = Sha256::new();
    hasher.update(cohort.as_bytes());
    hasher.update(b":");
    hasher.update(build.to_string().as_bytes());
    let digest = hasher.finalize();
    let bucket = u16::from_be_bytes([digest[0], digest[1]]) % 100;
    Ok(bucket < rollout as u16)
}

#[cfg(target_os = "windows")]
fn managed_package_paths(url: &str) -> ResultType<(PathBuf, PathBuf)> {
    let filename = url
        .split('/')
        .last()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty() && !s.contains('\\') && !s.contains('/'))
        .ok_or_else(|| hbb_common::anyhow::anyhow!("Invalid managed update package filename"))?;
    let dir = managed_update_dir().join("packages");
    fs::create_dir_all(&dir)?;
    let final_path = dir.join(filename);
    let part_path = dir.join(format!("{}.part", filename));
    Ok((final_path, part_path))
}

#[cfg(target_os = "windows")]
fn download_managed_package(
    payload: &ManagedManifestPayload,
    prefer_fallback: bool,
) -> ResultType<PathBuf> {
    let (final_path, part_path) = managed_package_paths(&payload.package.url)?;

    if final_path.exists()
        && verify_managed_package(
            &final_path,
            payload.package.size,
            &payload.package.sha256,
        )?
    {
        log::info!("Managed update package already downloaded and verified.");
        return Ok(final_path);
    }
    if final_path.exists() {
        fs::remove_file(&final_path).ok();
    }

    let sources = ordered_sources(&payload.package.url, prefer_fallback);
    let mut last_error = String::new();

    for (index, (url, is_fallback)) in sources.iter().enumerate() {
        match download_package_from_source(url, &part_path, payload.package.size) {
            Ok(()) => {
                if verify_managed_package(
                    &part_path,
                    payload.package.size,
                    &payload.package.sha256,
                )? {
                    if final_path.exists() {
                        fs::remove_file(&final_path)?;
                    }
                    fs::rename(&part_path, &final_path)?;
                    log::info!(
                        "Managed update package verified from {} source.",
                        if *is_fallback { "fallback" } else { "GitHub" }
                    );
                    return Ok(final_path);
                }

                last_error = format!("SHA-256 or size mismatch from {}", url);
                log::warn!("{}", last_error);
                fs::remove_file(&part_path).ok();
            }
            Err(e) => {
                last_error = e.to_string();
                log::warn!("Managed update download attempt failed: {}", e);
            }
        }

        if index + 1 < sources.len() {
            log::info!("Trying alternate managed update download source.");
        }
    }

    bail!("Failed to download managed update package: {}", last_error)
}

#[cfg(target_os = "windows")]
fn download_package_from_source(url: &str, part_path: &Path, expected_size: u64) -> ResultType<()> {
    let mut offset = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
    if offset > expected_size {
        fs::remove_file(part_path).ok();
        offset = 0;
    }
    if offset == expected_size {
        return Ok(());
    }

    let client = strict_update_client(MANAGED_PACKAGE_TIMEOUT)?;
    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={}-", offset));
    }

    let mut response = request.send()?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        bail!("Managed update package returned HTTP 404 from {}", url);
    }
    if !(status.is_success() || status == reqwest::StatusCode::PARTIAL_CONTENT) {
        bail!("Managed update package returned HTTP {} from {}", status, url);
    }

    let mut append = offset > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if append {
        let expected_prefix = format!("bytes {}-", offset);
        let content_range_ok = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with(&expected_prefix))
            .unwrap_or(false);
        if !content_range_ok {
            log::warn!(
                "Managed update source returned an unexpected Content-Range; restarting download."
            );
            append = false;
            offset = 0;
        }
    } else if offset > 0 {
        // Server ignored Range. Restart safely from byte zero.
        offset = 0;
    }

    let mut file = if append {
        OpenOptions::new().create(true).append(true).open(part_path)?
    } else {
        File::create(part_path)?
    };
    let copied = std::io::copy(&mut response, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    log::debug!(
        "Managed update downloaded {} bytes from {} (resume offset {}).",
        copied,
        url,
        offset
    );
    Ok(())
}

#[cfg(target_os = "windows")]
fn verify_managed_package(path: &Path, expected_size: u64, expected_sha256: &str) -> ResultType<bool> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != expected_size {
        return Ok(false);
    }

    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 128];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let actual = digest
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    Ok(actual.eq_ignore_ascii_case(expected_sha256))
}

#[cfg(target_os = "windows")]
fn install_managed_package(payload: &ManagedManifestPayload, package_path: &Path) -> ResultType<()> {
    let Some(path) = package_path.to_str() else {
        bail!(
            "Failed to convert managed update package path to string: {}",
            package_path.display()
        );
    };

    log::info!(
        "Installing RustDesk Managed {} build {} silently.",
        payload.version,
        payload.build
    );
    write_managed_state(Some(payload), true, None, "installing");

    match crate::platform::update_me_msi(path, true) {
        Ok(_) => {
            log::info!(
                "RustDesk Managed {} build {} installation launched successfully.",
                payload.version,
                payload.build
            );
            write_managed_state(Some(payload), true, None, "install-success");
            Ok(())
        }
        Err(e) => {
            let error = e.to_string();
            write_managed_state_with_error(
                Some(payload),
                true,
                None,
                "install-failed",
                Some(&error),
            );
            bail!(
                "Failed to install RustDesk Managed {} build {}: {}",
                payload.version,
                payload.build,
                error
            )
        }
    }
}

#[cfg(target_os = "windows")]
fn write_managed_state(
    payload: Option<&ManagedManifestPayload>,
    downloaded: bool,
    source: Option<&str>,
    result: &str,
) {
    write_managed_state_with_error(payload, downloaded, source, result, None);
}

#[cfg(target_os = "windows")]
fn write_managed_state_with_error(
    payload: Option<&ManagedManifestPayload>,
    downloaded: bool,
    source: Option<&str>,
    result: &str,
    error: Option<&str>,
) {
    let dir = managed_update_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        log::warn!("Failed to create managed update state directory: {}", e);
        return;
    }
    let channel = crate::managed_config::managed_update_channel();
    let state = ManagedUpdateState {
        current_version: crate::managed_config::MANAGED_VERSION,
        current_build: crate::managed_config::MANAGED_BUILD,
        channel: &channel,
        last_check_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        available_version: payload.map(|p| p.version.as_str()),
        available_build: payload.map(|p| p.build),
        downloaded,
        source,
        last_result: result,
        last_error: error,
    };
    match serde_json::to_vec_pretty(&state) {
        Ok(bytes) => {
            if let Err(e) = fs::write(dir.join("state.json"), bytes) {
                log::warn!("Failed to write managed update state: {}", e);
            }
        }
        Err(e) => log::warn!("Failed to serialize managed update state: {}", e),
    }
}

#[cfg(target_os = "windows")]
fn update_new_version(update_msi: bool, version: &str, file_path: &PathBuf) {
    log::debug!(
        "New version is downloaded, update begin, update msi: {update_msi}, version: {version}, file: {:?}",
        file_path.to_str()
    );
    if let Some(p) = file_path.to_str() {
        if let Some(session_id) = crate::platform::get_current_process_session_id() {
            if update_msi {
                match crate::platform::update_me_msi(p, true) {
                    Ok(_) => {
                        log::debug!("New version \"{}\" updated.", version);
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to install the new msi version  \"{}\": {}",
                            version,
                            e
                        );
                        std::fs::remove_file(&file_path).ok();
                    }
                }
            } else {
                let custom_client_staging_dir = if crate::is_custom_client() {
                    let custom_client_staging_dir =
                        crate::platform::get_custom_client_staging_dir();
                    if let Err(e) = crate::platform::handle_custom_client_staging_dir_before_update(
                        &custom_client_staging_dir,
                    ) {
                        log::error!(
                            "Failed to handle custom client staging dir before update: {}",
                            e
                        );
                        std::fs::remove_file(&file_path).ok();
                        return;
                    }
                    Some(custom_client_staging_dir)
                } else {
                    // Clean up any residual staging directory from previous custom client
                    let staging_dir = crate::platform::get_custom_client_staging_dir();
                    hbb_common::allow_err!(crate::platform::remove_custom_client_staging_dir(
                        &staging_dir
                    ));
                    None
                };
                let update_launched = match crate::platform::launch_privileged_process(
                    session_id,
                    &format!("{} --update", p),
                ) {
                    Ok(h) => {
                        if h.is_null() {
                            log::error!("Failed to update to the new version: {}", version);
                            false
                        } else {
                            log::debug!("New version \"{}\" is launched.", version);
                            true
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to run the new version: {}", e);
                        false
                    }
                };
                if !update_launched {
                    if let Some(dir) = custom_client_staging_dir {
                        hbb_common::allow_err!(crate::platform::remove_custom_client_staging_dir(
                            &dir
                        ));
                    }
                    std::fs::remove_file(&file_path).ok();
                }
            }
        } else {
            log::error!(
                "Failed to get the current process session id, Error {}",
                std::io::Error::last_os_error()
            );
            std::fs::remove_file(&file_path).ok();
        }
    } else {
        // unreachable!()
        log::error!(
            "Failed to convert the file path to string: {}",
            file_path.display()
        );
    }
}

pub fn get_update_download_file_from_url(url: &str) -> Option<PathBuf> {
    let parsed = url::Url::parse(url).ok()?;
    // Check the raw prefix before Url normalizes default ports.
    if !url.starts_with("https://github.com/")
        || parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }

    let mut segments = parsed.path_segments()?;
    let owner = segments.next()?;
    let repo = segments.next()?;
    let releases = segments.next()?;
    let download = segments.next()?;
    let tag = segments.next()?;
    let filename = segments.next()?;

    if owner != "rustdesk"
        || repo != "rustdesk"
        || releases != "releases"
        || download != "download"
        || tag.is_empty()
        || segments.next().is_some()
        || !is_plain_update_filename(filename)
    {
        return None;
    }

    Some(std::env::temp_dir().join(filename))
}

fn is_plain_update_filename(filename: &str) -> bool {
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains(':')
    {
        return false;
    }

    let mut components = Path::new(filename).components();
    matches!(
        components.next(),
        Some(Component::Normal(name)) if name.to_str() == Some(filename)
    ) && components.next().is_none()
}

pub fn get_download_file_from_url(url: &str) -> Option<PathBuf> {
    get_update_download_file_from_url(url)
}

/// Queries all active connections (remote, file-transfer, port-forward, camera, terminal)
/// from every logged-in user's --server process via IPC.
/// The root service cannot read connection state directly since connections
/// live in user --server processes. Handles fast user switching by querying
/// all GUI users, including the login-window server at UID 0. Falls back to
/// false (assumes sessions active) on any IPC error to avoid updating during
/// an unknown session state.
#[cfg(target_os = "macos")]
pub fn has_no_active_conns_ipc() -> bool {
    let rt = match hbb_common::tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return false,
    };
    rt.block_on(async {
        // Use the same GUI-domain-filtered UID set as the update script.
        // Shell-only SSH/TTY users are excluded, while an empty GUI set maps
        // to UID 0 so the LoginWindow server is queried rather than assumed idle.
        let uids = crate::platform::get_logged_in_uids();
        // Check each user's server — fail closed if any has active connections
        for uid in uids {
            if let Ok(mut conn) = crate::ipc::connect_for_uid(1000, uid, "").await {
                if conn.send(&crate::ipc::Data::HasNoActiveConns(None)).await.is_ok() {
                    match conn.next_timeout(1000).await {
                        Ok(Some(crate::ipc::Data::HasNoActiveConns(Some(true)))) => {
                            // Explicit no active connections — safe to continue
                        }
                        Ok(Some(crate::ipc::Data::HasNoActiveConns(Some(false)))) => {
                            return false; // Explicit active connections
                        }
                        _ => {
                            return false; // Timeout/error/unexpected — fail closed
                        }
                    }
                } else {
                    return false; // Send failed — fail closed
                }
            } else {
                return false; // Connection failed — fail closed
            }
        }
        true // All users explicitly confirmed no active connections
    })
}

#[cfg(target_os = "macos")]
fn wait_for_failed_update_retry() {
    const FAILURE_MARKER: &str = "/var/root/.rustdeskupdate_failed";
    let marker = std::path::Path::new(FAILURE_MARKER);
    if !marker.exists() {
        return;
    }

    // The updater script records failure immediately before launchd restarts
    // the old daemon. Preserve the retry deadline across that restart instead
    // of consuming the marker and retrying the same broken release in 30 sec.
    let remaining = std::fs::metadata(marker)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| {
            std::time::SystemTime::now()
                .duration_since(modified)
                .ok()
        })
        .map(|elapsed| RETRY_INTERVAL.saturating_sub(elapsed))
        .unwrap_or(RETRY_INTERVAL);
    if !remaining.is_zero() {
        log::info!(
            "[root-update] Previous update failed; retrying in {} seconds.",
            remaining.as_secs()
        );
        std::thread::sleep(remaining);
    }
    match std::fs::remove_file(marker) {
        Ok(()) => log::info!("[root-update] Previous update retry interval elapsed."),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => log::warn!("[root-update] Failed to clear failure marker: {}", err),
    }
}

/// Starts the background silent auto-update scheduler for macOS.
/// Called from `start_os_service()` which runs as root via LaunchDaemon.
#[cfg(target_os = "macos")]
pub fn start_auto_update_macos() {
    let spawn_result = std::thread::Builder::new()
        .name("rustdesk-auto-update".to_owned())
        .spawn(|| {
            log::info!("[root-update] Auto-update scheduler thread started.");
            std::thread::sleep(INITIAL_CHECK_DELAY);
            wait_for_failed_update_retry();
            let mut interval = DUR_ONE_DAY;
            loop {
                log::info!("[root-update] Running scheduled update check...");
                let no_active_conns = has_no_active_conns_ipc();
                if !no_active_conns {
                    log::info!("[root-update] Active session in progress, retrying in 10 min.");
                    interval = MIN_INTERVAL;
                } else {
                    match check_update_as_root() {
                        Ok(update_started) => {
                            if update_started {
                                // The replacement script is detached and may fail
                                // after this process returns. Always retry at the
                                // failure interval until the new daemon replaces us.
                                interval = RETRY_INTERVAL;
                            } else {
                                interval = DUR_ONE_DAY;
                            }
                        }
                        Err(e) => {
                            log::error!("[root-update] Update check failed: {}", e);
                            interval = RETRY_INTERVAL;
                        }
                    }
                }
                std::thread::sleep(interval);
            }
        });
    if let Err(err) = spawn_result {
        log::error!("[root-update] Failed to start scheduler thread: {}", err);
    }
}

#[cfg(target_os = "macos")]
pub fn check_update_as_root() -> ResultType<bool> {
    let _update_lock = acquire_mac_update_lock()?;
    // Allow-auto-update setting
    if !config::Config::get_bool_option(keys::OPTION_ALLOW_AUTO_UPDATE) {
        log::info!("[root-update] Auto update is disabled, skipping.");
        return Ok(false);
    }
    if crate::is_custom_client() {
        log::info!("[root-update] Custom client detected, skipping stock update.");
        return Ok(false);
    }
    // Clean up only old temp dirs from previous failed updates. The detached
    // installer keeps using its update directory after this process exits and
    // releases the advisory lock, so a newly-started daemon must not remove a
    // directory that still belongs to the active transaction.
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(".rustdeskupdate-root-")
                || name_str.starts_with(".rustdeskdownload-")
            {
                let path = entry.path();
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                let mode = metadata.mode() & 0o7777;
                let is_stale = metadata
                    .modified()
                    .ok()
                    .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
                    .is_some_and(|age| age >= RETRY_INTERVAL);
                if metadata.file_type().is_dir() && metadata.uid() == 0 && mode == 0o700 && is_stale
                {
                    if let Err(err) = std::fs::remove_dir_all(&path) {
                        log::warn!(
                            "[root-update] Failed to remove stale temp dir {}: {}",
                            path.display(),
                            err
                        );
                    }
                }
            }
        }
    }
    if let Err(e) = do_check_software_update() {
        bail!("[root-update] Failed to check for software update: {}", e);
    }
    let update_url = crate::common::SOFTWARE_UPDATE_URL.lock().unwrap().clone();
    if update_url.is_empty() {
        log::info!("[root-update] No update available.");
        return Ok(false);
    }
    let download_url = update_url.replace("tag", "download");
    let version = download_url.split('/').last().unwrap_or_default().to_string();
    let arch = if std::env::consts::ARCH == "aarch64" { "aarch64" } else { "x86_64" };
    let dmg_url = format!("{}/rustdesk-{}-{}.dmg", download_url, version, arch);
    log::info!("[root-update] New version: {}, downloading from {}", version, dmg_url);
    // Validate URL against GitHub release allowlist before downloading as root
    let Some(file_path_validated) = get_update_download_file_from_url(&dmg_url) else {
        bail!("[root-update] URL failed allowlist check: {}", dmg_url);
    };
    drop(file_path_validated);
    let client = create_http_client_with_url_strict(&dmg_url)?;
    // Use mktemp so a local user cannot pre-create a predictable path and
    // permanently deny updates for a reused service PID.
    let private_tmp_output = std::process::Command::new("/usr/bin/mktemp")
        .args(["-d", "/tmp/.rustdeskdownload-XXXXXX"])
        .output()?;
    if !private_tmp_output.status.success() {
        bail!(
            "[root-update] Failed to create private download directory: {}",
            String::from_utf8_lossy(&private_tmp_output.stderr).trim()
        );
    }
    let private_tmp = String::from_utf8(private_tmp_output.stdout)
        .map_err(|err| hbb_common::anyhow::anyhow!("[root-update] mktemp output error: {}", err))?
        .trim()
        .to_owned();
    if private_tmp.is_empty() {
        bail!("[root-update] mktemp returned an empty download directory");
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private_tmp, std::fs::Permissions::from_mode(0o700))?;
    }
    let filename = dmg_url.split('/').last().unwrap_or("rustdesk.dmg");
    let file_path = std::path::PathBuf::from(format!("{}/{}", private_tmp, filename));
    let tmp_path = file_path.to_string_lossy().to_string();
    // Download
    let mut response = client.get(&dmg_url).send()?;
    if !response.status().is_success() {
        let _ = std::fs::remove_dir_all(&private_tmp);
        bail!("[root-update] Failed to download: {}", response.status());
    }
    // Create file exclusively (O_EXCL) and stream response directly into it
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file_path)
            .map_err(|e| { let _ = std::fs::remove_dir_all(&private_tmp); e })?;
        std::io::copy(&mut response, &mut file)
            .map_err(|e| { let _ = std::fs::remove_dir_all(&private_tmp); e })?;
    }
    log::info!("[root-update] Downloaded to {}", tmp_path);
    // Recheck active sessions before installing — download can take minutes
    if !has_no_active_conns_ipc() {
        if let Err(e) = std::fs::remove_dir_all(&private_tmp) {
            log::warn!("[root-update] Failed to remove temp dir {}: {}", private_tmp, e);
        }
        bail!("[root-update] Active session started during download, deferring update.");
    }
    // Install silently as root
    let result = crate::platform::update_from_dmg_as_root(&tmp_path, &version);
    // Clean up download directory
    if let Err(e) = std::fs::remove_dir_all(&private_tmp) {
        log::warn!("[root-update] Failed to remove temp dir {}: {}", private_tmp, e);
    }
    result.map(|_| true)
}

#[cfg(test)]
mod tests {
    use super::get_download_file_from_url;

    #[test]
    fn update_download_file_accepts_expected_github_asset_urls() {
        let file = get_download_file_from_url(
            "https://github.com/rustdesk/rustdesk/releases/download/1.4.0/rustdesk-1.4.0-x86_64.dmg",
        )
        .expect("valid GitHub release asset URL");

        assert_eq!(
            file.file_name().and_then(|name| name.to_str()),
            Some("rustdesk-1.4.0-x86_64.dmg")
        );
    }

    #[test]
    fn update_download_file_rejects_untrusted_or_malformed_urls() {
        for url in [
            "http://github.com/rustdesk/rustdesk/releases/download/1/rustdesk.exe",
            "https://example.com/rustdesk.exe",
            "https://github.com/other/project/releases/download/1/rustdesk.exe",
            "https://github.com/rustdesk/rustdesk/releases/download/1/",
            "https://github.com/rustdesk/rustdesk/releases/download/1/nested/rustdesk.exe",
            "https://github.com/rustdesk/rustdesk/releases/download/1/C:rustdesk.exe",
            "https://user@github.com/rustdesk/rustdesk/releases/download/1/rustdesk.exe",
            "https://github.com:443/rustdesk/rustdesk/releases/download/1/rustdesk.exe",
            "https://github.com/rustdesk/rustdesk/releases/download/1/rustdesk.exe?download=1",
            "https://github.com/rustdesk/rustdesk/releases/download/1/rustdesk.exe#download",
            "not a url",
        ] {
            assert!(get_download_file_from_url(url).is_none(), "{url}");
        }
    }
}
