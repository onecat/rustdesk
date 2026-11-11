# RustDesk Managed 1.5.1-r1 migration notes

## Source and scope
- Upstream tag: `rustdesk/rustdesk@1.5.1` (`23e5bb471a040d62f0a6ad52591fb30784265b73`).
- Previous released Cat Managed tag: `managed-1.5.0-r1` (`f9b9502dcc3bf59adfe7c4f7f6abb32cb9f20920`, Build 1014).
- Target: `1.5.1-r1`, Build 1015.
- Existing Managed server, password, dashboard, diagnostics, LAN, MSI and updater customizations were migrated.
- Upstream and Cat edits to overlapping files were merged against the common 1.5.0 ancestor. The updater needed manual import/insertion reconciliation so that upstream artifact verification and the Cat update channel both remain.

## New requested defaults
- Global default view style: `adaptive` (适应窗口).
- Global default scroll style: `scrollbar` (滚动条).
- Implemented with `DEFAULT_DISPLAY_SETTINGS`, not `OVERWRITE_DISPLAY_SETTINGS`, to preserve a user's ability to choose a different style.
- Existing saved per-peer preferences can still override global defaults. An existing explicit global preference is also retained.

## Windows version reporting
- The Cat dashboard reads `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
- Uses the actual build number to display Windows 11 even if `ProductName` still says Windows 10. Build >= 22000 indicates Windows 11 only for Windows 10-branded product names; Windows Server is not relabeled.
- Displays edition, `DisplayVersion`, build and UBR where available.
- Falls back to Dart's `Platform.operatingSystemVersion` if the registry query fails.
- Pure formatting tests cover Windows 10, Windows 11, and Windows Server.

## Required verification before publication
1. CI build compiles upstream 1.5.1 with Managed customizations, produces MSI and passes checksum and source-SHA validation.
2. Smoke-test new installation and upgrades from the released 1.5.0-r1 MSI; verify service autostart, permanent and administration credentials, settings/uninstall protection, and stable server lock.
3. Verify the Display settings show `适应窗口` and `滚动条` for new defaults and that the options remain user-editable; confirm existing per-peer preferences remain effective.
4. Verify the Dashboard's edition and build on Windows 10 and Windows 11, including build 22000+.
5. Test dual Cat LAN discovery and direct IP:21118 connectivity across multi-NIC, NAT and side-router environments, and interaction with official client.
6. Test automatic updates against a staged manifest, SHA-256 verification, resumable download, partial rollout and installation only when no remote sessions are active. Verify signed upstream updater functionality is retained outside Managed handling.
7. Recheck installer security and Windows protected update handoff changed upstream in 1.5.1.
8. Keep 1.5.0-r1 release and Stable manifest intact until successful verification and deliberate publication.

Do not publish `managed-1.5.1-r1` or update the stable manifest from this branch before verification.
