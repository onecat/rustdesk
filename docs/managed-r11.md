# RustDesk Managed 1.4.9-r11

R11 focuses on LAN management, diagnostics, and update visibility.

Implemented scope:

- Managed LAN page using RustDesk native UDP discovery.
- One-way discovery policy: Cat can scan peers while remaining non-discoverable.
- LAN peer cache with online/offline state, IP/MAC merge, and persisted last-seen timestamps.
- Search by hostname, username, IP, MAC, or RustDesk ID plus online/offline filters.
- Manual discovery and 60-second automatic scanning while the app window is visible.
- Per-peer Remote Control, File Transfer, Terminal, and Wake-on-LAN actions.
- Advanced Windows network adapter details: IPv4, MAC, gateway, DNS, adapter type, and status.
- Diagnostics for ID Server, Relay, API, local Direct port, DNS resolution, and latency.
- Read-only Managed server configuration details.
- Managed update detail page with current/latest build, last check, source, download/install state, and failure reason.

Managed administration tabs:

`概览 | 连接 | 文件 | 终端 | 局域网 | 诊断`

Version metadata:

- Managed version: 1.4.9-r11
- Managed build: 1011
- MSI ProductVersion target: 1.4.9.30001011
