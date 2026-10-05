use hbb_common::{
    config::{self, keys, Config},
    lazy_static,
    sysinfo::{Disks, System},
};
use std::sync::Mutex;

pub(crate) const MANAGED_VERSION: &str = "1.4.9-r11";
pub(crate) const MANAGED_BUILD: u64 = 1011;
pub(crate) const MANAGED_UPDATE_MANIFEST_BASE: &str =
    "https://github.com/onecat/rustdesk/releases/download/managed-update";
pub(crate) const MANAGED_GITHUB_FALLBACK_PREFIX: &str = "https://gh.catmak.name/";

pub(crate) fn managed_updates_enabled() -> bool {
    cfg!(target_os = "windows")
}

pub(crate) fn managed_update_channel() -> String {
    "stable".to_owned()
}

pub(crate) fn managed_update_manifest_url() -> String {
    format!(
        "{}/{}.json",
        MANAGED_UPDATE_MANIFEST_BASE,
        managed_update_channel()
    )
}

lazy_static::lazy_static! {
    /// Reuse one sysinfo instance so CPU utilization has a previous sample and
    /// the dashboard does not repeatedly rebuild expensive system state.
    static ref MANAGED_SYSTEM_INFO: Mutex<System> = Mutex::new(System::new());
    static ref MANAGED_DISKS: Mutex<Disks> = Mutex::new(Disks::new());
}

/// Lightweight system snapshot for the Cat dashboard.
///
/// Memory values follow the sysinfo 0.29 representation and are passed through
/// unchanged; the Flutter side formats them defensively. Disk enumeration is
/// intentionally optional so the common 10-second refresh does not rescan
/// volumes. The UI requests disk data only on first display and every 60 seconds.
pub(crate) fn managed_system_info_json(refresh_disk: bool) -> String {
    let mut sys = match MANAGED_SYSTEM_INFO.lock() {
        Ok(sys) => sys,
        Err(_) => return "{}".to_owned(),
    };

    sys.refresh_cpu();
    sys.refresh_memory();

    let cpu_usage = sys.global_cpu_info().cpu_usage();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_owned())
        .unwrap_or_default();

    let system_drive = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".to_owned())
        .to_lowercase();
    let (disk_total, disk_available) = match MANAGED_DISKS.lock() {
        Ok(mut disks) => {
            if refresh_disk {
                disks.refresh_list();
                disks.refresh();
            }
            disks
                .list()
                .iter()
                .find(|disk| {
                    disk.mount_point()
                        .to_string_lossy()
                        .to_lowercase()
                        .starts_with(&system_drive)
                })
                .map(|disk| (disk.total_space(), disk.available_space()))
                .unwrap_or((0, 0))
        }
        Err(_) => (0, 0),
    };

    serde_json::json!({
        "cpu_usage": cpu_usage,
        "cpu_brand": cpu_brand,
        "memory_total": sys.total_memory(),
        "memory_used": sys.used_memory(),
        "uptime": sys.uptime(),
        "disk_total": disk_total,
        "disk_available": disk_available,
    })
    .to_string()
}

/// Rich LAN peer cache for the Cat R11 management page. RustDesk's native
/// discovery code already marks cached peers offline before each scan and
/// merges multiple IP/MAC pairs for the same peer.
pub(crate) fn managed_lan_peers_json() -> String {
    let peers: Vec<serde_json::Value> = config::LanPeers::load()
        .peers
        .into_iter()
        .map(|peer| {
            serde_json::json!({
                "id": peer.id,
                "hostname": peer.hostname,
                "username": peer.username,
                "platform": peer.platform,
                "online": peer.online,
                "ip_mac": peer.ip_mac,
            })
        })
        .collect();
    serde_json::to_string(&peers).unwrap_or_else(|_| "[]".to_owned())
}

/// Expose the enforced Managed endpoints read-only to the R11 diagnostics UI.
pub(crate) fn managed_server_config_json() -> String {
    serde_json::json!({
        "rendezvous": "rustdesk-server.catmak.name",
        "rendezvous_port": 21116,
        "relay": "rustdesk-relay.catmak.name",
        "relay_port": 21117,
        "api": "rustdesk-api.catmak.name",
        "api_port": 443,
        "direct_port": 21118,
        "lan_discovery_reply": false,
    })
    .to_string()
}



/// This build intentionally locks its private-server policy.
pub(crate) fn server_settings_locked() -> bool {
    true
}

/// Version checks against the public RustDesk update service are disabled.
pub(crate) fn version_check_disabled() -> bool {
    true
}

/// Cat Managed keeps the incoming-session permission window hidden on Windows.
/// Session visibility is retained through the tray tooltip/session count.
pub(crate) fn connection_manager_hidden() -> bool {
    cfg!(target_os = "windows")
}

/// Detect the Windows secure/locked desktop without treating ordinary focus
/// changes or minimization as a lock event. A secure desktop also covers UAC
/// secure prompts, where relocking the local management UI is conservative.
#[cfg(target_os = "windows")]
pub(crate) fn windows_session_locked() -> bool {
    use winapi::um::winuser::{
        CloseDesktop, OpenInputDesktop, SwitchDesktop, DESKTOP_SWITCHDESKTOP,
    };

    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
        if desktop.is_null() {
            return true;
        }
        let unlocked = SwitchDesktop(desktop) != 0;
        CloseDesktop(desktop);
        !unlocked
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn windows_session_locked() -> bool {
    false
}

/// Read password-derived material injected only into the final Windows build.
///
/// The public repository never contains the plaintext permanent password.
/// GitHub Actions derives the authentication hash from the password secret and
/// the public, stable Managed salt, then injects both at compile time.
#[cfg(target_os = "windows")]
fn installed_password_material(hash_name: &str, salt_name: &str) -> Option<(String, String)> {
    use winreg::{
        enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY},
        RegKey,
    };

    let root = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = root
        .open_subkey_with_flags(
            r"SOFTWARE\RustDesk\Managed",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .ok()?;
    let storage: String = key.get_value(hash_name).ok()?;
    let salt: String = key.get_value(salt_name).ok()?;
    if storage.starts_with("00") && storage.len() > 2 && !salt.is_empty() {
        Some((storage, salt))
    } else {
        None
    }
}

#[cfg(not(target_os = "windows"))]
fn installed_password_material(_hash_name: &str, _salt_name: &str) -> Option<(String, String)> {
    None
}

fn preset_password_material() -> Option<(String, String)> {
    if let Some(material) =
        installed_password_material("RemotePasswordStorage", "RemotePasswordSalt")
    {
        return Some(material);
    }

    let storage = option_env!("RUSTDESK_FIXED_PASSWORD_HASH").unwrap_or("");
    let salt = option_env!("RUSTDESK_FIXED_PASSWORD_SALT").unwrap_or("");
    if storage.starts_with("00") && storage.len() > 2 && !salt.is_empty() {
        Some((storage.to_owned(), salt.to_owned()))
    } else {
        None
    }
}

/// Verify the managed settings/uninstall password against the same
/// build-time material used by RustDesk permanent-password authentication.
/// No plaintext password is embedded in the public source or final binary.
pub(crate) fn verify_fixed_password(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    let Some((storage, salt)) = preset_password_material() else {
        return false;
    };
    let Some(expected) = config::decode_preset_password_h1_from_storage(&storage) else {
        return false;
    };
    let actual = config::compute_permanent_password_h1(input, &salt);

    // Constant-time comparison for the fixed-size SHA-256 value.
    let mut diff = 0u8;
    for i in 0..actual.len() {
        diff |= actual[i] ^ expected[i];
    }
    diff == 0
}

/// Read the dedicated Cat dashboard administrator credential injected at build time.
///
/// Managed builds support a separate RUSTDESK_ADMIN_PASSWORD repository secret. The CI
/// pipeline may temporarily fall back to the remote-access secret if the new
/// secret has not been configured yet, allowing a non-breaking migration.
fn preset_admin_password_material() -> Option<(String, String)> {
    if let Some(material) =
        installed_password_material("AdminPasswordStorage", "AdminPasswordSalt")
    {
        return Some(material);
    }

    let storage = option_env!("RUSTDESK_ADMIN_PASSWORD_HASH").unwrap_or("");
    let salt = option_env!("RUSTDESK_ADMIN_PASSWORD_SALT").unwrap_or("");
    if storage.starts_with("00") && storage.len() > 2 && !salt.is_empty() {
        Some((storage.to_owned(), salt.to_owned()))
    } else {
        None
    }
}

/// Validate the local Cat management-mode password without exposing plaintext.
pub(crate) fn verify_admin_password(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    let Some((storage, salt)) = preset_admin_password_material() else {
        return false;
    };
    let Some(expected) = config::decode_preset_password_h1_from_storage(&storage) else {
        return false;
    };
    let actual = config::compute_permanent_password_h1(input, &salt);

    let mut diff = 0u8;
    for i in 0..actual.len() {
        diff |= actual[i] ^ expected[i];
    }
    diff == 0
}

/// Apply managed's enforced client policy after any external/custom configuration.
pub(crate) fn apply() {
    let has_preset_password = if let Some((storage, salt)) = preset_password_material() {
        {
            let mut hard = config::HARD_SETTINGS.write().unwrap();
            hard.insert("password".to_owned(), storage);
            hard.insert("salt".to_owned(), salt);
        }

        // Remove an old locally persisted password so the injected preset password
        // remains authoritative when upgrading an existing RustDesk installation.
        if !Config::is_disable_change_permanent_password() {
            let _ = Config::set_permanent_password("");
        }
        true
    } else {
        false
    };

    {
        // Hide account-related pages/features in this managed client.
        config::HARD_SETTINGS
            .write()
            .unwrap()
            .insert("disable-account".to_owned(), "Y".to_owned());
    }

    {
        let mut settings = config::OVERWRITE_SETTINGS.write().unwrap();
        for (key, value) in [
            (
                keys::OPTION_CUSTOM_RENDEZVOUS_SERVER,
                "rustdesk-server.catmak.name",
            ),
            (keys::OPTION_RELAY_SERVER, "rustdesk-relay.catmak.name"),
            (
                keys::OPTION_API_SERVER,
                "https://rustdesk-api.catmak.name",
            ),
            (
                keys::OPTION_KEY,
                "LmNVkrH1xApjMUUdBggKBz9NCieG+jBS8te9pmrdZcQ=",
            ),
            (keys::OPTION_ALLOW_AUTO_UPDATE, "N"),
            (keys::OPTION_ENABLE_LAN_DISCOVERY, "N"),
            (keys::OPTION_DIRECT_SERVER, "Y"),
            (keys::OPTION_DIRECT_ACCESS_PORT, "21118"),
            (keys::OPTION_ALLOW_REMOTE_CONFIG_MODIFICATION, "Y"),
            ("stop-service", "N"),
        ] {
            settings.insert(key.to_owned(), value.to_owned());
        }
        if has_preset_password {
            settings.insert(keys::OPTION_APPROVE_MODE.to_owned(), "password".to_owned());
            settings.insert(
                keys::OPTION_VERIFICATION_METHOD.to_owned(),
                "use-permanent-password".to_owned(),
            );
        }
    }

    {
        let mut local = config::OVERWRITE_LOCAL_SETTINGS.write().unwrap();
        local.insert(keys::OPTION_ENABLE_CHECK_UPDATE.to_owned(), "N".to_owned());
    }

    {
        let mut builtin = config::BUILTIN_SETTINGS.write().unwrap();
        for (key, value) in [
            (keys::OPTION_HIDE_SERVER_SETTINGS, "Y"),
            (keys::OPTION_HIDE_HELP_CARDS, "Y"),
            (keys::OPTION_HIDE_STOP_SERVICE, "Y"),
            (keys::OPTION_HIDE_REMOTE_PRINTER_SETTINGS, "Y"),
            (keys::OPTION_ALLOW_DEEP_LINK_SERVER_SETTINGS, "N"),
        ] {
            builtin.insert(key.to_owned(), value.to_owned());
        }
        if has_preset_password {
            builtin.insert(
                keys::OPTION_REMOVE_PRESET_PASSWORD_WARNING.to_owned(),
                "Y".to_owned(),
            );
            builtin.insert(
                keys::OPTION_DISABLE_CHANGE_PERMANENT_PASSWORD.to_owned(),
                "Y".to_owned(),
            );
        }
    }
}
