# Managed 1.5.3-r2 / Build 1017 — manual update check fix

## Symptom (observed in published Build 1016)

On the Managed diagnostics page, clicking **检查更新** did not always update **上次检查**.

## Root causes in Build 1016

1. The updater's random 120–600-second initial sleep prevented an explicitly requested check from being serviced immediately after startup.
2. The 10-minute `MIN_INTERVAL` scheduler throttle applied to manual and automatic requests alike, silently ignoring manual requests made shortly after an automatic check.
3. The Flutter diagnostics button waited only 12 seconds for a timestamp update, although the manifest request and alternate-source fallback can run longer. The button could stop spinning while the check was still underway.

`last_check_unix` is the last **completed / reported** check time, not the time at which a request was placed. It advances only when the Managed updater writes its state after the network check, including error reporting.

## Fix (development branch only)

- Use `recv_timeout(initial_delay)` instead of blocking `sleep` so a manual request interrupts the initial randomized automatic delay.
- Allow `UpdateMsg::CheckUpdate` to bypass the scheduler's 10-minute automatic-check throttle. Preserve automatic scheduling and the existing remote-session safeguards.
- Keep the diagnostics button busy during the asynchronous check and poll for the actual updated `last_check_unix` for up to 90 seconds; report a visible incomplete/error message rather than silently reverting to an idle button.
- Add a pure Rust regression test for manual-vs-automatic minimum-interval decisions.

## Verification checklist

1. Windows MSI Build 1017 compiles and produces an artifact from `rustdesk-managed-1.5.3-r2` (no release publication).
2. Immediately after launching Windows RustDesk, open Diagnostics, click **检查更新**: the manual check must not wait for the 2–10-minute startup delay.
3. Click **检查更新** twice less than 10 minutes apart: both requests should be processed, even though automatic checks remain rate limited.
4. When GitHub is slow or the primary manifest endpoint fails, the UI should stay on **正在检查更新…** while polling and later show **已是最新版本** / **检查失败** / an explicit timeout message.
5. Validate `上次检查` time reflects the completed attempt. The UI must not fake a successful check by simply setting its own wall-clock time.
6. Confirm regular six-hour scheduled checks, stable channel, SHA-256 validation, fallback URLs, active-session install deferral, and the existing Build 1016 Stable manifest are unchanged.

This branch is a candidate for `1.5.3-r2 / Build 1017` and is **not** a modification of the immutable `1.5.3-r1` Release.
