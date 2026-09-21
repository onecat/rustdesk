use crate::{common::do_check_software_update, hbbs_http::create_http_client_with_url};
use hbb_common::{bail, config, log, ResultType};
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver, Sender},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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

const DUR_ONE_DAY: Duration = Duration::from_secs(60 * 60 * 24);
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
fn has_no_active_conns() -> bool {
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
        // Spread update checks across machines. The value is intentionally local
        // and does not identify the device to the update server.
        120 + (hbb_common::rand::random::<u64>() % 481)
    } else {
        30
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
            log::error!("Error checking for updates: {}", e);
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
                        log::error!("Error checking for updates: {}", e);
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
    #[cfg(target_os = "windows")]
    let update_msi = crate::platform::is_msi_installed()? && !crate::is_custom_client();
    if !(manually || config::Config::get_bool_option(config::keys::OPTION_ALLOW_AUTO_UPDATE)) {
        return Ok(());
    }
    if do_check_software_update().is_err() {
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
        let client = create_http_client_with_url(&download_url);
        let Some(file_path) = get_download_file_from_url(&download_url) else {
            bail!("Failed to get the file path from the URL: {}", download_url);
        };
        let mut is_file_exists = false;
        if file_path.exists() {
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
            write_managed_state(Some(payload), true, None, "install-failed");
            bail!(
                "Failed to install RustDesk Managed {} build {}: {}",
                payload.version,
                payload.build,
                e
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
        log::error!(
            "Failed to convert the file path to string: {}",
            file_path.display()
        );
    }
}

pub fn get_download_file_from_url(url: &str) -> Option<PathBuf> {
    let filename = url.split('/').last()?;
    Some(std::env::temp_dir().join(filename))
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn managed_fallback_url_is_only_for_github() {
        assert_eq!(
            github_fallback_url("https://github.com/onecat/rustdesk/releases/download/x/a.msi"),
            Some("https://gh.catmak.name/https://github.com/onecat/rustdesk/releases/download/x/a.msi".to_owned())
        );
        assert_eq!(github_fallback_url("https://example.com/a.msi"), None);
    }

    #[test]
    fn managed_rollout_bounds_are_stable() {
        assert!(selected_for_rollout(100, 1005).unwrap());
        assert!(!selected_for_rollout(0, 1005).unwrap());
    }
}
