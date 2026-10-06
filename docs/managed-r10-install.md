# RustDesk Managed 1.5.0-r1 command-line installation

1.5.0-r1 supports optional per-machine credential overrides during MSI installation.

## Silent install

```powershell
msiexec /i rustdesk-managed-1.5.0-r1-x86_64.msi /qn REMOTE_PASSWORD="<remote-password>" ADMIN_PASSWORD="<management-password>"
```

- `REMOTE_PASSWORD` overrides the permanent remote-access password for this machine.
- `ADMIN_PASSWORD` overrides the local Cat management-mode password for this machine.
- The properties are independent; either one may be omitted.
- If an override is omitted on a fresh install, the build-time Managed default is used.
- If an override is omitted during an upgrade, an existing machine override is preserved.
- An explicit uninstall removes the machine-specific overrides after password verification.

The MSI properties are authored as hidden and secure. The installer persists only password-derived material and salts under the machine-wide Managed registry key; it does not persist the supplied plaintext values.

Because command-line arguments can be observed by privileged local software while `msiexec` is running, deployment systems should avoid exposing command lines to untrusted users.


## Release

The stable 1.5.0-r1 package is published from the exact CI artifact built from the release-trigger commit. The release tag is `managed-1.5.0-r1`.
