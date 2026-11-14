# Cat RustDesk Managed 1.5.3-r1 — migration and verification

## Source of truth

- Upstream stable tag: `rustdesk/rustdesk@1.5.3`, commit `95cca3f661f635b3cc85c04935fe8d96d8f432ef`.
- Previous Cat integration branch: `rustdesk-managed-1.5.1-r1`, commit `5953e65d186a89695cbe55faecbc0f2f2d0c2472`.
- New isolated integration branch: `rustdesk-managed-1.5.3-r1`, version `1.5.3-r1`, Build `1016`.
- Existing public release `managed-1.5.0-r1` / Build `1014` is unchanged; do not modify the Stable manifest as part of this migration.

## Migration

The base is the **official 1.5.3 release tree**, not a copy of 1.5.1. The Cat-only Rust/Flutter/MSI/update/CI sources have been reapplied. The eight overlapping files have been merged from the common official 1.5.1 ancestor:

- `flutter/lib/desktop/pages/desktop_home_page.dart`
- `src/common.rs`, `src/core_main.rs`, `src/flutter_ffi.rs`, `src/platform/windows.rs`
- `src/lang/cn.rs`, `src/lang/en.rs`, `src/lang/tw.rs`

Retained Cat behavior: private server lock, permanent and administrator password behavior, Managed Dashboard, LAN discovery and IP:21118 direct connection, diagnostics, independently controlled Managed stable updates, MSI custom actions and uninstall protection.

Retained previous 1.5.1-r1 fixes: default view style `adaptive` (适应窗口), default scroll style `scrollbar` (滚动条), and the Windows 10/11 edition/build-based version display using the Windows CurrentVersion registry data. These are user-overridable defaults, not forced overrides. Previously saved per-peer/global choices still apply.

The upstream 1.5.3 Windows installer changes, IPC authentication, relay destination validation and request-approval feature must remain functional. Pay special attention to the interaction between upstream install handoff, Cat's custom MSI update path, private-server lock and permanent password policy.

## Required checks (not implied by merging)

1. Confirm `cargo` and Flutter compile successfully with the 1.5.3 dependencies and the Managed bridge; build the Windows x64 MSI in GitHub Actions with `publish_release=false`.
2. Verify new installs and in-place upgrades from Cat 1.5.0-r1: service autostart, tray, no unexpected wizard or desktop shortcut, remote password and admin password overrides, blocked unattended uninstall, password-authenticated interactive uninstall.
3. On Windows 10 and Windows 11, verify Dashboard displays correct edition, marketing release and NT build. Check display defaults on a fresh profile; saved user preferences must not be reset or forcibly overridden.
4. Test managed server lock and fallback behavior; do not permit an upstream/public relay or update channel to replace the Cat endpoint.
5. Test two Managed peers LAN discovery in both directions, correct peer IP in a multi-NIC / side-router setup, direct `IP:21118` connection, and an official peer interoperability case.
6. Verify managed manifest schema, channel, rollout cohort, SHA-256, fallback downloader, active-connection deferral, and new secure MSI handoff.
7. Regression-test clipboard, screen capture, request approval / automatic authentication, service IPC reliability, relay fallback and port forwarding.
8. Perform review of the actual build logs and installer artifacts before creating an immutable release. Do **not** publish Stable updates or modify existing release artifacts until these tests pass.

## Known scope limitation

A source merge or structural check is **not** a successful build or Windows integration test. Until a corresponding GitHub Actions run produces and validates an MSI, version 1.5.3-r1 remains **unverified/unreleased**.
