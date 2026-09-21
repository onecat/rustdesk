use hbb_common::config::{self, keys, Config};

pub(crate) const MANAGED_VERSION: &str = "1.4.9-r4";
pub(crate) const MANAGED_BUILD: u64 = 1004;
pub(crate) const MANAGED_UPDATE_MANIFEST_BASE: &str =
    "https://github.com/onecat/rustdesk/releases/download/managed-update";
pub(crate) const MANAGED_GITHUB_FALLBACK_PREFIX: &str = "https://gh.catmak.name/";

pub(crate) fn managed_updates_enabled() -> bool {
    cfg!(target_os = "windows")
}

pub(crate) fn managed_update_channel() -> String {
    let configured = Config::get_option("managed-update-channel");
    if configured.eq_ignore_ascii_case("test") {
        "test".to_owned()
    } else {
        "stable".to_owned()
    }
}

pub(crate) fn managed_update_manifest_url() -> String {
    format!(
        "{}/{}.json",
        MANAGED_UPDATE_MANIFEST_BASE,
        managed_update_channel()
    )
}


/// This build intentionally locks its private-server policy.
pub(crate) fn server_settings_locked() -> bool {
    true
}

/// Version checks against the public RustDesk update service are disabled.
pub(crate) fn version_check_disabled() -> bool {
    true
}

/// Read password-derived material injected only into the final Windows build.
///
/// The public repository contains neither the plaintext permanent password nor
/// its authentication hash/salt. GitHub Actions supplies these compile-time
/// environment variables from the repository secret.
fn preset_password_material() -> Option<(&'static str, &'static str)> {
    let storage = option_env!("RUSTDESK_FIXED_PASSWORD_HASH").unwrap_or("");
    let salt = option_env!("RUSTDESK_FIXED_PASSWORD_SALT").unwrap_or("");
    if storage.starts_with("00") && storage.len() > 2 && !salt.is_empty() {
        Some((storage, salt))
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
    let Some(expected) = config::decode_preset_password_h1_from_storage(storage) else {
        return false;
    };
    let actual = config::compute_permanent_password_h1(input, salt);

    // Constant-time comparison for the fixed-size SHA-256 value.
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
            hard.insert("password".to_owned(), storage.to_owned());
            hard.insert("salt".to_owned(), salt.to_owned());
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
