import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/desktop/pages/connection_page.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:window_manager/window_manager.dart';

class ManagedDashboardPage extends StatefulWidget {
  const ManagedDashboardPage({Key? key}) : super(key: key);

  @override
  State<ManagedDashboardPage> createState() => _ManagedDashboardPageState();
}

class _ManagedDashboardPageState extends State<ManagedDashboardPage> {
  Timer? _systemTimer;
  Timer? _ipTimer;

  bool _adminUnlocked = false;
  int _tabIndex = 0;
  bool _passwordSet = false;

  String _version = '';
  String _build = '';
  String _computerName = '';
  String _userName = '';
  String _windowsVersion = '';
  String _ip = '';

  String _cpuBrand = '';
  double _cpuUsage = 0;
  int _memoryTotalBytes = 0;
  int _memoryUsedBytes = 0;
  int _diskTotalBytes = 0;
  int _diskAvailableBytes = 0;
  int _uptimeSeconds = 0;
  DateTime _lastDiskRefresh = DateTime.fromMillisecondsSinceEpoch(0);

  @override
  void initState() {
    super.initState();
    windowManager.setTitle('RustDesk - Cat 定制版');
    gFFI.serverModel.addListener(_onServerModelChanged);
    _loadStaticInfo();
    _refreshIp();
    _refreshSystemInfo();
    _systemTimer =
        Timer.periodic(const Duration(seconds: 10), (_) => _refreshSystemInfo());
    _ipTimer = Timer.periodic(const Duration(seconds: 10), (_) => _refreshIp());
  }

  void _onServerModelChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  @override
  void dispose() {
    gFFI.serverModel.removeListener(_onServerModelChanged);
    _systemTimer?.cancel();
    _ipTimer?.cancel();
    super.dispose();
  }

  Future<bool> _shouldRefresh() async {
    try {
      return await windowManager.isVisible() && !await windowManager.isMinimized();
    } catch (_) {
      return true;
    }
  }

  Future<void> _loadStaticInfo() async {
    final version = await bind.mainGetCommon(key: 'managed-version');
    final build = await bind.mainGetCommon(key: 'managed-build');
    final passwordSet =
        (await bind.mainGetCommon(key: 'permanent-password-set')) == 'true';
    if (!mounted) return;
    setState(() {
      _computerName = Platform.localHostname;
      _userName =
          Platform.environment['USERNAME'] ?? Platform.environment['USER'] ?? '--';
      _windowsVersion = Platform.operatingSystemVersion;
      _version = version;
      _build = build;
      _passwordSet = passwordSet;
    });
  }

  bool _isPrivateIpv4(InternetAddress address) {
    final bytes = address.rawAddress;
    if (bytes.length != 4) return false;
    return bytes[0] == 10 ||
        (bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31) ||
        (bytes[0] == 192 && bytes[1] == 168);
  }

  int _ipScore(NetworkInterface interface, InternetAddress address) {
    final name = interface.name.toLowerCase();
    var score = _isPrivateIpv4(address) ? 50 : 0;
    if (name.contains('wi-fi') ||
        name.contains('wifi') ||
        name.contains('wlan') ||
        name.contains('ethernet')) {
      score += 100;
    }
    if (name.contains('virtual') ||
        name.contains('vmware') ||
        name.contains('vbox') ||
        name.contains('hyper-v') ||
        name.contains('vethernet') ||
        name.contains('docker') ||
        name.contains('wsl') ||
        name.contains('tailscale') ||
        name.contains('zerotier') ||
        name.contains('bluetooth')) {
      score -= 100;
    }
    return score;
  }

  Future<void> _refreshIp() async {
    if (!mounted || !await _shouldRefresh()) return;
    try {
      final interfaces = await NetworkInterface.list(
        type: InternetAddressType.IPv4,
        includeLoopback: false,
        includeLinkLocal: false,
      );
      final candidates = <MapEntry<int, String>>[];
      for (final interface in interfaces) {
        for (final address in interface.addresses) {
          if (address.type != InternetAddressType.IPv4 ||
              address.isLoopback ||
              address.isLinkLocal ||
              address.address == '0.0.0.0') {
            continue;
          }
          candidates.add(MapEntry(_ipScore(interface, address), address.address));
        }
      }
      candidates.sort((a, b) {
        final score = b.key.compareTo(a.key);
        return score != 0 ? score : a.value.compareTo(b.value);
      });
      final value = candidates.isEmpty ? '' : candidates.first.value;
      if (mounted && value != _ip) {
        setState(() => _ip = value);
      }
    } catch (e) {
      debugPrint('Failed to enumerate Cat dashboard IPv4: ' + e.toString());
    }
  }

  Future<void> _refreshSystemInfo() async {
    if (!mounted || !await _shouldRefresh()) return;
    try {
      final refreshDisk =
          DateTime.now().difference(_lastDiskRefresh) >= const Duration(seconds: 60);
      final raw = await bind.mainGetCommon(
        key: refreshDisk
            ? 'managed-system-info-with-disk'
            : 'managed-system-info',
      );
      if (raw.isEmpty) return;
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic> || !mounted) return;

      setState(() {
        _cpuUsage = (decoded['cpu_usage'] as num?)?.toDouble() ?? _cpuUsage;
        final brand = decoded['cpu_brand']?.toString() ?? '';
        if (brand.isNotEmpty) _cpuBrand = brand;
        _memoryTotalBytes =
            (decoded['memory_total'] as num?)?.toInt() ?? _memoryTotalBytes;
        _memoryUsedBytes =
            (decoded['memory_used'] as num?)?.toInt() ?? _memoryUsedBytes;
        _uptimeSeconds =
            (decoded['uptime'] as num?)?.toInt() ?? _uptimeSeconds;
        final diskTotal = (decoded['disk_total'] as num?)?.toInt() ?? 0;
        final diskAvailable = (decoded['disk_available'] as num?)?.toInt() ?? 0;
        if (diskTotal > 0) {
          _diskTotalBytes = diskTotal;
          _diskAvailableBytes = diskAvailable;
          _lastDiskRefresh = DateTime.now();
        }
      });
    } catch (e) {
      debugPrint('Failed to refresh Cat dashboard system info: ' + e.toString());
    }
  }

  String _formatMemory(int bytes) {
    return _formatBytes(bytes);
  }

  String _formatBytes(int bytes) {
    if (bytes <= 0) return '--';
    final gib = bytes / 1024 / 1024 / 1024;
    return gib.toStringAsFixed(gib >= 10 ? 1 : 2) + ' GB';
  }

  String _formatUptime(int seconds) {
    if (seconds <= 0) return '--';
    final days = seconds ~/ 86400;
    final hours = (seconds % 86400) ~/ 3600;
    final minutes = (seconds % 3600) ~/ 60;
    if (days > 0) return days.toString() + '天 ' + hours.toString() + '小时';
    if (hours > 0) {
      return hours.toString() + '小时 ' + minutes.toString() + '分钟';
    }
    return minutes.toString() + '分钟';
  }

  double _percent(int used, int total) {
    if (total <= 0) return 0;
    return (used / total * 100).clamp(0, 100).toDouble();
  }

  Future<void> _copy(String value) async {
    if (value.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: value));
    showToast(translate('Copied'));
  }

  Future<void> _unlock() async {
    final controller = TextEditingController();
    var errorText = '';
    var busy = false;
    await showDialog<void>(
      context: context,
      builder: (dialogContext) {
        return StatefulBuilder(
          builder: (context, setDialogState) {
            Future<void> submit() async {
              if (controller.text.isEmpty || busy) return;
              setDialogState(() {
                busy = true;
                errorText = '';
              });
              final ok = await bind.mainVerifyManagedAdminPassword(
                  password: controller.text);
              if (!ok) {
                await Future.delayed(const Duration(milliseconds: 500));
                if (dialogContext.mounted) {
                  setDialogState(() {
                    busy = false;
                    errorText = '密码错误';
                  });
                }
                return;
              }
              if (mounted) {
                setState(() {
                  _adminUnlocked = true;
                  _tabIndex = 1;
                });
              }
              if (dialogContext.mounted) Navigator.of(dialogContext).pop();
            }

            return AlertDialog(
              title: const Row(
                children: [
                  Icon(Icons.lock_open_rounded),
                  SizedBox(width: 10),
                  Text('管理模式验证'),
                ],
              ),
              content: SizedBox(
                width: 360,
                child: TextField(
                  controller: controller,
                  obscureText: true,
                  autofocus: true,
                  enabled: !busy,
                  onSubmitted: (_) => submit(),
                  decoration: InputDecoration(
                    labelText: '管理密码',
                    hintText: '输入密码以显示连接功能',
                    errorText: errorText.isEmpty ? null : errorText,
                  ),
                ),
              ),
              actions: [
                TextButton(
                  onPressed:
                      busy ? null : () => Navigator.of(dialogContext).pop(),
                  child: const Text('取消'),
                ),
                FilledButton.icon(
                  onPressed: busy ? null : submit,
                  icon: busy
                      ? const SizedBox(
                          width: 14,
                          height: 14,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        )
                      : const Icon(Icons.lock_open_rounded, size: 18),
                  label: const Text('解锁'),
                ),
              ],
            );
          },
        );
      },
    );
    controller.dispose();
  }

  void _lock() {
    setState(() {
      _adminUnlocked = false;
      _tabIndex = 0;
    });
  }

  Widget _statusDot(bool ok, String okText, String badText) {
    final color = ok ? const Color(0xFF2E9B65) : const Color(0xFFD9534F);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(Icons.circle, size: 10, color: color),
        const SizedBox(width: 7),
        Text(ok ? okText : badText),
      ],
    );
  }

  Widget _card(BuildContext context, String title, IconData icon, Widget child) {
    return Card(
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(
        side: BorderSide(color: Theme.of(context).dividerColor.withOpacity(0.35)),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(icon, size: 20, color: MyTheme.accent),
                const SizedBox(width: 8),
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 16),
            child,
          ],
        ),
      ),
    );
  }

  Widget _row(String label, String value,
      {Widget? trailing, bool emphasize = false}) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Row(
        children: [
          SizedBox(
            width: 112,
            child: Text(
              label,
              style: TextStyle(
                color: Theme.of(context)
                    .textTheme
                    .bodyMedium
                    ?.color
                    ?.withOpacity(0.62),
              ),
            ),
          ),
          Expanded(
            child: SelectableText(
              value.isEmpty ? '--' : value,
              maxLines: 2,
              style: TextStyle(
                fontSize: emphasize ? 17 : 14,
                fontWeight: emphasize ? FontWeight.w600 : FontWeight.w400,
              ),
            ),
          ),
          if (trailing != null) trailing,
        ],
      ),
    );
  }

  Widget _copyButton(String value) {
    return IconButton(
      tooltip: '复制',
      visualDensity: VisualDensity.compact,
      onPressed: value.isEmpty ? null : () => _copy(value),
      icon: const Icon(Icons.copy_rounded, size: 18),
    );
  }

  Widget _metric(String label, String value, double percent) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 13),
      child: Column(
        children: [
          Row(
            children: [
              Expanded(child: Text(label)),
              Text(value, style: const TextStyle(fontWeight: FontWeight.w600)),
            ],
          ),
          const SizedBox(height: 6),
          LinearProgressIndicator(
            value: (percent / 100).clamp(0, 1),
            minHeight: 6,
            borderRadius: BorderRadius.circular(5),
          ),
        ],
      ),
    );
  }

  Widget _dashboard(BuildContext context) {
    final id = gFFI.serverModel.serverId.text;
    final connectStatus = gFFI.serverModel.connectStatus;
    final memoryPercent = _percent(_memoryUsedBytes, _memoryTotalBytes);
    final diskUsed =
        _diskTotalBytes > 0 ? _diskTotalBytes - _diskAvailableBytes : 0;
    final diskPercent = _percent(diskUsed, _diskTotalBytes);

    return SingleChildScrollView(
      padding: const EdgeInsets.all(24),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final twoColumns = constraints.maxWidth >= 840;
          final width =
              twoColumns ? (constraints.maxWidth - 16) / 2 : constraints.maxWidth;
          return Wrap(
            spacing: 16,
            runSpacing: 16,
            children: [
              SizedBox(
                width: width,
                child: _card(
                  context,
                  '设备信息',
                  Icons.computer_rounded,
                  Column(
                    children: [
                      _row('计算机名', _computerName, emphasize: true),
                      _row('用户名', _userName),
                      _row('Windows', _windowsVersion),
                      _row('Cat 版本', _version),
                      _row('Build', _build),
                    ],
                  ),
                ),
              ),
              SizedBox(
                width: width,
                child: _card(
                  context,
                  '远程访问',
                  Icons.security_rounded,
                  Column(
                    children: [
                      _row('RustDesk ID', id,
                          emphasize: true, trailing: _copyButton(id)),
                      _row('固定访问密码', _passwordSet ? '已启用' : '未配置'),
                      _row('IP Address', _ip.isEmpty ? '--' : _ip,
                          emphasize: true, trailing: _copyButton(_ip)),
                      _row('Direct Port', '21118'),
                    ],
                  ),
                ),
              ),
              SizedBox(
                width: width,
                child: _card(
                  context,
                  '系统状态',
                  Icons.monitor_heart_outlined,
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (_cpuBrand.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 12),
                          child: Text(
                            _cpuBrand,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: Theme.of(context)
                                  .textTheme
                                  .bodySmall
                                  ?.color
                                  ?.withOpacity(0.7),
                            ),
                          ),
                        ),
                      _metric('CPU',
                          _cpuUsage.toStringAsFixed(0) + '%', _cpuUsage),
                      _metric(
                        '内存',
                        _formatMemory(_memoryUsedBytes) +
                            ' / ' +
                            _formatMemory(_memoryTotalBytes),
                        memoryPercent,
                      ),
                      _metric(
                        '系统盘',
                        _diskTotalBytes <= 0
                            ? '--'
                            : _formatBytes(diskUsed) +
                                ' / ' +
                                _formatBytes(_diskTotalBytes),
                        diskPercent,
                      ),
                      _row('运行时间', _formatUptime(_uptimeSeconds)),
                    ],
                  ),
                ),
              ),
              SizedBox(
                width: width,
                child: _card(
                  context,
                  '运行状态',
                  Icons.health_and_safety_outlined,
                  Column(
                    children: [
                      _row(
                        'RustDesk Service',
                        '',
                        trailing: _statusDot(
                          bind.mainGetOptionSync(key: 'stop-service') != 'Y',
                          '正常运行',
                          '服务停止',
                        ),
                      ),
                      _row(
                        'Server',
                        '',
                        trailing: _statusDot(
                          connectStatus == 1,
                          '已连接',
                          connectStatus == 0 ? '连接中' : '连接异常',
                        ),
                      ),
                      _row(
                        '更新通道',
                        'Stable' + (_version.isEmpty ? '' : ' / ' + _version),
                      ),
                      const SizedBox(height: 10),
                      Row(
                        children: [
                          OutlinedButton.icon(
                            onPressed: id.isEmpty ? null : () => _copy(id),
                            icon: const Icon(Icons.badge_outlined, size: 18),
                            label: const Text('复制 ID'),
                          ),
                          const SizedBox(width: 8),
                          OutlinedButton.icon(
                            onPressed: _ip.isEmpty ? null : () => _copy(_ip),
                            icon: const Icon(Icons.lan_outlined, size: 18),
                            label: const Text('复制 IP'),
                          ),
                          const Spacer(),
                          IconButton(
                            tooltip: '刷新状态',
                            onPressed: () {
                              _refreshIp();
                              _refreshSystemInfo();
                            },
                            icon: const Icon(Icons.refresh_rounded),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final connectStatus = gFFI.serverModel.connectStatus;
    return Container(
      color: Theme.of(context).scaffoldBackgroundColor,
      child: Column(
        children: [
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 14),
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.background,
              border: Border(
                bottom: BorderSide(
                  color: Theme.of(context).dividerColor.withOpacity(0.35),
                ),
              ),
            ),
            child: Row(
              children: [
                const Icon(
                  Icons.desktop_windows_rounded,
                  color: MyTheme.accent,
                  size: 24,
                ),
                const SizedBox(width: 10),
                const Text(
                  'RustDesk - Cat 定制版',
                  style: TextStyle(fontSize: 19, fontWeight: FontWeight.w600),
                ),
                const SizedBox(width: 14),
                _statusDot(
                  connectStatus == 1,
                  '在线',
                  connectStatus == 0 ? '连接中' : '网络异常',
                ),
                const Spacer(),
                if (_adminUnlocked)
                  TextButton.icon(
                    onPressed: _lock,
                    icon: const Icon(Icons.lock_open_rounded, size: 19),
                    label: const Text('管理模式'),
                  )
                else
                  Tooltip(
                    message: '进入管理模式',
                    child: IconButton(
                      onPressed: _unlock,
                      icon: const Icon(Icons.lock_rounded),
                    ),
                  ),
              ],
            ),
          ),
          if (_adminUnlocked)
            Container(
              padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
              alignment: Alignment.centerLeft,
              child: SegmentedButton<int>(
                segments: const [
                  ButtonSegment<int>(
                    value: 0,
                    icon: Icon(Icons.dashboard_outlined),
                    label: Text('概览'),
                  ),
                  ButtonSegment<int>(
                    value: 1,
                    icon: Icon(Icons.laptop_chromebook_rounded),
                    label: Text('连接'),
                  ),
                ],
                selected: {_tabIndex},
                showSelectedIcon: false,
                onSelectionChanged: (selection) {
                  setState(() => _tabIndex = selection.first);
                },
              ),
            ),
          Expanded(
            child: _adminUnlocked && _tabIndex == 1
                ? const Padding(
                    padding: EdgeInsets.only(top: 8),
                    child: ConnectionPage(),
                  )
                : _dashboard(context),
          ),
        ],
      ),
    );
  }
}
