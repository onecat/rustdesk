import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:window_manager/window_manager.dart';

class ManagedLanPage extends StatefulWidget {
  const ManagedLanPage({Key? key}) : super(key: key);

  @override
  State<ManagedLanPage> createState() => _ManagedLanPageState();
}

class _ManagedLanPeer {
  final String id;
  final String hostname;
  final String username;
  final String platform;
  final bool online;
  final Map<String, String> ipMac;
  final DateTime? lastSeen;

  const _ManagedLanPeer({
    required this.id,
    required this.hostname,
    required this.username,
    required this.platform,
    required this.online,
    required this.ipMac,
    required this.lastSeen,
  });
}

class _ManagedLanPageState extends State<ManagedLanPage> {
  static const _lastSeenOption = 'managed-r11-lan-last-seen';
  final TextEditingController _searchController = TextEditingController();
  final Map<String, DateTime> _lastSeen = {};
  Timer? _timer;

  List<_ManagedLanPeer> _peers = const [];
  String _statusFilter = 'all';
  bool _scanning = false;
  String _lastScanText = '--';

  @override
  void initState() {
    super.initState();
    _loadLastSeen();
    _loadPeers();
    Future.delayed(const Duration(milliseconds: 350), () {
      if (mounted) _scan();
    });
    _timer = Timer.periodic(const Duration(seconds: 60), (_) => _scan());
  }

  @override
  void dispose() {
    _timer?.cancel();
    _searchController.dispose();
    super.dispose();
  }

  void _loadLastSeen() {
    try {
      final raw = bind.getLocalFlutterOption(k: _lastSeenOption);
      if (raw.isEmpty) return;
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic>) return;
      for (final entry in decoded.entries) {
        final unix = (entry.value as num?)?.toInt() ?? 0;
        if (unix > 0) {
          _lastSeen[entry.key] =
              DateTime.fromMillisecondsSinceEpoch(unix * 1000);
        }
      }
    } catch (e) {
      debugPrint('Failed to load R11 LAN last-seen cache: $e');
    }
  }

  Future<void> _saveLastSeen() async {
    final data = <String, int>{};
    for (final entry in _lastSeen.entries) {
      data[entry.key] = entry.value.millisecondsSinceEpoch ~/ 1000;
    }
    try {
      await bind.setLocalFlutterOption(
        k: _lastSeenOption,
        v: jsonEncode(data),
      );
    } catch (e) {
      debugPrint('Failed to save R11 LAN last-seen cache: $e');
    }
  }

  Future<bool> _windowCanScan() async {
    try {
      return await windowManager.isVisible() &&
          !await windowManager.isMinimized();
    } catch (_) {
      return true;
    }
  }

  Future<void> _loadPeers() async {
    try {
      final raw = await bind.mainGetCommon(key: 'managed-lan-peers');
      final decoded = jsonDecode(raw);
      if (decoded is! List) return;

      final now = DateTime.now();
      final peers = <_ManagedLanPeer>[];
      for (final item in decoded) {
        if (item is! Map<String, dynamic>) continue;
        final id = item['id']?.toString() ?? '';
        if (id.isEmpty) continue;

        final ipMac = <String, String>{};
        final rawIpMac = item['ip_mac'];
        if (rawIpMac is Map) {
          for (final entry in rawIpMac.entries) {
            ipMac[entry.key.toString()] = entry.value.toString();
          }
        }

        final online = item['online'] == true;
        if (online) _lastSeen[id] = now;
        peers.add(_ManagedLanPeer(
          id: id,
          hostname: item['hostname']?.toString() ?? '',
          username: item['username']?.toString() ?? '',
          platform: item['platform']?.toString() ?? '',
          online: online,
          ipMac: ipMac,
          lastSeen: online ? now : _lastSeen[id],
        ));
      }

      peers.sort((a, b) {
        if (a.online != b.online) return a.online ? -1 : 1;
        final ah = a.hostname.toLowerCase();
        final bh = b.hostname.toLowerCase();
        final hostCompare = ah.compareTo(bh);
        return hostCompare != 0 ? hostCompare : a.id.compareTo(b.id);
      });

      if (!mounted) return;
      setState(() => _peers = peers);
      await _saveLastSeen();
    } catch (e) {
      debugPrint('Failed to load R11 LAN peers: $e');
    }
  }

  Future<void> _scan() async {
    if (_scanning || !await _windowCanScan()) return;
    if (!mounted) return;
    setState(() => _scanning = true);

    try {
      bind.mainDiscover();
      for (var i = 0; i < 4; i++) {
        await Future.delayed(const Duration(seconds: 1));
        if (!mounted) return;
        await _loadPeers();
      }
      if (mounted) {
        final now = DateTime.now();
        setState(() {
          _lastScanText =
              '${now.hour.toString().padLeft(2, '0')}:${now.minute.toString().padLeft(2, '0')}:${now.second.toString().padLeft(2, '0')}';
        });
      }
    } catch (e) {
      debugPrint('R11 LAN discovery failed: $e');
    } finally {
      if (mounted) setState(() => _scanning = false);
    }
  }

  String _formatLastSeen(DateTime? time) {
    if (time == null) return '未记录';
    final now = DateTime.now();
    final diff = now.difference(time);
    if (diff.inMinutes < 1) return '刚刚';
    if (diff.inHours < 1) return '${diff.inMinutes} 分钟前';
    if (diff.inDays < 1) return '${diff.inHours} 小时前';
    return '${time.year}-${time.month.toString().padLeft(2, '0')}-${time.day.toString().padLeft(2, '0')} '
        '${time.hour.toString().padLeft(2, '0')}:${time.minute.toString().padLeft(2, '0')}';
  }

  bool _matches(_ManagedLanPeer peer) {
    if (_statusFilter == 'online' && !peer.online) return false;
    if (_statusFilter == 'offline' && peer.online) return false;

    final q = _searchController.text.trim().toLowerCase();
    if (q.isEmpty) return true;
    final haystack = <String>[
      peer.id,
      peer.hostname,
      peer.username,
      peer.platform,
      ...peer.ipMac.keys,
      ...peer.ipMac.values,
    ].join(' ').toLowerCase();
    return haystack.contains(q);
  }

  Future<void> _openPeer(
    _ManagedLanPeer peer, {
    bool fileTransfer = false,
    bool terminal = false,
  }) async {
    await connect(
      context,
      peer.id,
      isFileTransfer: fileTransfer,
      isTerminal: terminal,
    );
  }

  Widget _peerCard(_ManagedLanPeer peer) {
    final ips = peer.ipMac.keys.toList()..sort();
    final macs = peer.ipMac.values.where((e) => e.isNotEmpty).toSet().toList()
      ..sort();

    return Card(
      elevation: 0,
      margin: const EdgeInsets.only(bottom: 10),
      shape: RoundedRectangleBorder(
        side: BorderSide(
          color: Theme.of(context).dividerColor.withOpacity(0.35),
        ),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            Icon(
              peer.online ? Icons.circle : Icons.circle_outlined,
              size: 12,
              color: peer.online
                  ? const Color(0xFF2E9B65)
                  : Theme.of(context).disabledColor,
            ),
            const SizedBox(width: 12),
            SizedBox(
              width: 190,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    peer.hostname.isEmpty ? peer.id : peer.hostname,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontWeight: FontWeight.w600),
                  ),
                  const SizedBox(height: 3),
                  SelectableText(
                    peer.id,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ],
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Wrap(
                spacing: 22,
                runSpacing: 6,
                children: [
                  _detail('用户', peer.username.isEmpty ? '--' : peer.username),
                  _detail('IP', ips.isEmpty ? '--' : ips.join(', ')),
                  _detail('MAC', macs.isEmpty ? '--' : macs.join(', ')),
                  _detail('系统', peer.platform.isEmpty ? '--' : peer.platform),
                  _detail('最后发现', _formatLastSeen(peer.lastSeen)),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Wrap(
              spacing: 4,
              children: [
                IconButton(
                  tooltip: '连接',
                  onPressed: () => _openPeer(peer),
                  icon: const Icon(Icons.desktop_windows_rounded),
                ),
                IconButton(
                  tooltip: '文件',
                  onPressed: () => _openPeer(peer, fileTransfer: true),
                  icon: const Icon(Icons.folder_copy_outlined),
                ),
                IconButton(
                  tooltip: '终端',
                  onPressed: () => _openPeer(peer, terminal: true),
                  icon: const Icon(Icons.terminal_rounded),
                ),
                IconButton(
                  tooltip: macs.isEmpty ? '没有可用 MAC 地址' : 'Wake-on-LAN',
                  onPressed:
                      macs.isEmpty ? null : () => bind.mainWol(id: peer.id),
                  icon: const Icon(Icons.power_settings_new_rounded),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _detail(String label, String value) {
    return SizedBox(
      width: 170,
      child: RichText(
        overflow: TextOverflow.ellipsis,
        text: TextSpan(
          style: DefaultTextStyle.of(context).style,
          children: [
            TextSpan(
              text: '$label: ',
              style: TextStyle(
                color: Theme.of(context)
                    .textTheme
                    .bodyMedium
                    ?.color
                    ?.withOpacity(0.55),
              ),
            ),
            TextSpan(text: value),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final visiblePeers = _peers.where(_matches).toList();
    final onlineCount = _peers.where((p) => p.online).length;

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 18, 24, 12),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  controller: _searchController,
                  onChanged: (_) => setState(() {}),
                  decoration: const InputDecoration(
                    prefixIcon: Icon(Icons.search_rounded),
                    labelText: '搜索局域网设备',
                    hintText: '设备名 / 用户名 / IP / RustDesk ID',
                    border: OutlineInputBorder(),
                    isDense: true,
                  ),
                ),
              ),
              const SizedBox(width: 12),
              SegmentedButton<String>(
                segments: const [
                  ButtonSegment(value: 'all', label: Text('全部')),
                  ButtonSegment(value: 'online', label: Text('在线')),
                  ButtonSegment(value: 'offline', label: Text('离线')),
                ],
                selected: {_statusFilter},
                showSelectedIcon: false,
                onSelectionChanged: (v) =>
                    setState(() => _statusFilter = v.first),
              ),
              const SizedBox(width: 12),
              FilledButton.icon(
                onPressed: _scanning ? null : _scan,
                icon: _scanning
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.radar_rounded, size: 18),
                label: Text(_scanning ? '扫描中' : '立即扫描'),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 0, 24, 10),
          child: Row(
            children: [
              Text('发现 ${_peers.length} 台 · 在线 $onlineCount 台'),
              const SizedBox(width: 16),
              Text(
                '上次扫描：$_lastScanText',
                style: Theme.of(context).textTheme.bodySmall,
              ),
              const Spacer(),
              const Icon(Icons.visibility_off_outlined, size: 16),
              const SizedBox(width: 5),
              const Text('单向发现：本机不响应其他设备扫描'),
              const SizedBox(width: 12),
              const Text('自动刷新：60 秒'),
            ],
          ),
        ),
        Expanded(
          child: visiblePeers.isEmpty
              ? Center(
                  child: Text(
                    _scanning ? '正在发现局域网设备…' : '没有符合条件的局域网设备',
                  ),
                )
              : ListView.builder(
                  padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
                  itemCount: visiblePeers.length,
                  itemBuilder: (_, index) => _peerCard(visiblePeers[index]),
                ),
        ),
      ],
    );
  }
}
