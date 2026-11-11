import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';

class ManagedDiagnosticsPage extends StatefulWidget {
  const ManagedDiagnosticsPage({Key? key}) : super(key: key);

  @override
  State<ManagedDiagnosticsPage> createState() => _ManagedDiagnosticsPageState();
}

class _ProbeResult {
  final String name;
  final String host;
  final int port;
  final bool ok;
  final String address;
  final int elapsedMs;
  final String error;

  const _ProbeResult({
    required this.name,
    required this.host,
    required this.port,
    required this.ok,
    required this.address,
    required this.elapsedMs,
    required this.error,
  });
}

class _ManagedDiagnosticsPageState extends State<ManagedDiagnosticsPage> {
  bool _loadingNetwork = false;
  bool _runningDiagnostics = false;
  bool _checkingUpdate = false;

  List<Map<String, dynamic>> _interfaces = const [];
  List<_ProbeResult> _probes = const [];
  Map<String, dynamic> _serverConfig = const {};
  Map<String, dynamic> _updateState = const {};
  String _networkError = '';
  DateTime? _diagnosticTime;

  @override
  void initState() {
    super.initState();
    _refreshAll();
  }

  Future<void> _refreshAll() async {
    await Future.wait([
      _loadServerConfig(),
      _loadNetworkInfo(),
      _loadUpdateState(),
    ]);
    await _runDiagnostics();
  }

  Future<void> _loadServerConfig() async {
    try {
      final raw = await bind.mainGetCommon(key: 'managed-server-config');
      final decoded = jsonDecode(raw);
      if (decoded is Map<String, dynamic> && mounted) {
        setState(() => _serverConfig = decoded);
      }
    } catch (e) {
      debugPrint('Failed to load R11 server config: $e');
    }
  }

  String _networkType(String alias, String description) {
    final text = '$alias $description'.toLowerCase();
    if (text.contains('virtual') ||
        text.contains('vmware') ||
        text.contains('vbox') ||
        text.contains('hyper-v') ||
        text.contains('vethernet') ||
        text.contains('docker') ||
        text.contains('wsl') ||
        text.contains('tailscale') ||
        text.contains('zerotier')) {
      return 'Virtual';
    }
    if (text.contains('wi-fi') ||
        text.contains('wifi') ||
        text.contains('wireless') ||
        text.contains('wlan')) {
      return 'Wi-Fi';
    }
    if (text.contains('ethernet')) return 'Ethernet';
    return 'Other';
  }

  Future<void> _loadNetworkInfo() async {
    if (_loadingNetwork) return;
    setState(() {
      _loadingNetwork = true;
      _networkError = '';
    });

    const script = r'''
$items = Get-NetIPConfiguration | ForEach-Object {
  $adapter = $_.NetAdapter
  [PSCustomObject]@{
    alias = [string]$_.InterfaceAlias
    description = [string]$_.InterfaceDescription
    status = if ($adapter) { [string]$adapter.Status } else { "" }
    mac = if ($adapter) { [string]$adapter.MacAddress } else { "" }
    ipv4 = [string](@($_.IPv4Address | ForEach-Object { $_.IPAddress }) -join ", ")
    gateway = [string](@($_.IPv4DefaultGateway | ForEach-Object { $_.NextHop }) -join ", ")
    dns = [string](@($_.DNSServer.ServerAddresses) -join ", ")
  }
}
$items | ConvertTo-Json -Compress
''';

    try {
      final result = await Process.run(
        'powershell.exe',
        const [
          '-NoProfile',
          '-NonInteractive',
          '-ExecutionPolicy',
          'Bypass',
          '-Command',
          script,
        ],
      ).timeout(const Duration(seconds: 8));

      if (result.exitCode != 0) {
        throw Exception(result.stderr.toString().trim());
      }

      final text = result.stdout.toString().trim();
      final decoded = text.isEmpty ? <dynamic>[] : jsonDecode(text);
      final items = decoded is List ? decoded : [decoded];
      final interfaces = <Map<String, dynamic>>[];
      for (final item in items) {
        if (item is! Map) continue;
        final map = Map<String, dynamic>.from(item);
        final alias = map['alias']?.toString() ?? '';
        final description = map['description']?.toString() ?? '';
        map['type'] = _networkType(alias, description);
        interfaces.add(map);
      }
      interfaces.sort((a, b) {
        final aUp = (a['status']?.toString().toLowerCase() == 'up') ? 0 : 1;
        final bUp = (b['status']?.toString().toLowerCase() == 'up') ? 0 : 1;
        if (aUp != bUp) return aUp.compareTo(bUp);
        return (a['alias']?.toString() ?? '')
            .compareTo(b['alias']?.toString() ?? '');
      });

      if (mounted) setState(() => _interfaces = interfaces);
    } catch (e) {
      if (mounted) {
        setState(() => _networkError = e.toString());
      }
    } finally {
      if (mounted) setState(() => _loadingNetwork = false);
    }
  }

  Future<_ProbeResult> _tcpProbe(
    String name,
    String host,
    int port,
  ) async {
    final watch = Stopwatch()..start();
    String address = '';
    try {
      final addresses =
          await InternetAddress.lookup(host).timeout(const Duration(seconds: 3));
      if (addresses.isEmpty) throw const SocketException('DNS 无结果');
      final preferred = addresses.firstWhere(
        (a) => a.type == InternetAddressType.IPv4,
        orElse: () => addresses.first,
      );
      address = preferred.address;

      final socket = await Socket.connect(
        preferred,
        port,
        timeout: const Duration(seconds: 3),
      );
      socket.destroy();
      watch.stop();
      return _ProbeResult(
        name: name,
        host: host,
        port: port,
        ok: true,
        address: address,
        elapsedMs: watch.elapsedMilliseconds,
        error: '',
      );
    } catch (e) {
      watch.stop();
      return _ProbeResult(
        name: name,
        host: host,
        port: port,
        ok: false,
        address: address,
        elapsedMs: watch.elapsedMilliseconds,
        error: e.toString(),
      );
    }
  }

  Future<_ProbeResult> _localDirectProbe(int port) async {
    final watch = Stopwatch()..start();
    try {
      final socket = await Socket.connect(
        InternetAddress.loopbackIPv4,
        port,
        timeout: const Duration(seconds: 2),
      );
      socket.destroy();
      watch.stop();
      return _ProbeResult(
        name: 'Direct',
        host: '127.0.0.1',
        port: port,
        ok: true,
        address: '127.0.0.1',
        elapsedMs: watch.elapsedMilliseconds,
        error: '',
      );
    } catch (e) {
      watch.stop();
      return _ProbeResult(
        name: 'Direct',
        host: '127.0.0.1',
        port: port,
        ok: false,
        address: '127.0.0.1',
        elapsedMs: watch.elapsedMilliseconds,
        error: e.toString(),
      );
    }
  }

  int _intConfig(String key, int fallback) =>
      (_serverConfig[key] as num?)?.toInt() ?? fallback;

  String _stringConfig(String key, String fallback) =>
      _serverConfig[key]?.toString() ?? fallback;

  Future<void> _runDiagnostics() async {
    if (_runningDiagnostics) return;
    if (_serverConfig.isEmpty) await _loadServerConfig();
    if (!mounted) return;

    setState(() => _runningDiagnostics = true);
    final rendezvous =
        _stringConfig('rendezvous', 'rustdesk-server.catmak.name');
    final relay = _stringConfig('relay', 'rustdesk-relay.catmak.name');
    final api = _stringConfig('api', 'rustdesk-api.catmak.name');

    final results = await Future.wait([
      _tcpProbe('ID Server', rendezvous, _intConfig('rendezvous_port', 21116)),
      _tcpProbe('Relay', relay, _intConfig('relay_port', 21117)),
      _tcpProbe('API', api, _intConfig('api_port', 443)),
      _localDirectProbe(_intConfig('direct_port', 21118)),
    ]);

    if (mounted) {
      setState(() {
        _probes = results;
        _diagnosticTime = DateTime.now();
        _runningDiagnostics = false;
      });
    }
  }

  Future<void> _loadUpdateState() async {
    try {
      final raw = await bind.mainGetCommon(key: 'managed-update-state');
      final decoded = jsonDecode(raw);
      if (decoded is Map<String, dynamic> && mounted) {
        setState(() => _updateState = decoded);
      }
    } catch (e) {
      debugPrint('Failed to load R11 update state: $e');
    }
  }

  Future<void> _checkUpdate() async {
    if (_checkingUpdate) return;
    setState(() => _checkingUpdate = true);
    final before = (_updateState['last_check_unix'] as num?)?.toInt() ?? 0;
    try {
      await bind.mainManagedCheckUpdate();
      for (var i = 0; i < 12; i++) {
        await Future.delayed(const Duration(seconds: 1));
        if (!mounted) return;
        await _loadUpdateState();
        final after =
            (_updateState['last_check_unix'] as num?)?.toInt() ?? 0;
        if (after > before) break;
      }
    } catch (e) {
      debugPrint('Failed to request R11 update check: $e');
    } finally {
      if (mounted) setState(() => _checkingUpdate = false);
    }
  }

  String _updateResultText(String value) {
    switch (value) {
      case 'checked':
        return '已检查';
      case 'up-to-date':
        return '已是最新版本';
      case 'rollout-wait':
        return '等待分批推送';
      case 'downloaded':
        return '已下载';
      case 'waiting-for-idle':
        return '已下载，等待空闲安装';
      case 'installing':
        return '正在安装';
      case 'install-success':
        return '升级程序已启动';
      case 'install-failed':
        return '安装失败';
      case 'check-failed':
        return '检查失败';
      case 'not-checked':
        return '尚未检查';
      default:
        return value.isEmpty ? '--' : value;
    }
  }

  String _formatUnix(dynamic value) {
    final unix = (value as num?)?.toInt() ?? 0;
    if (unix <= 0) return '--';
    final t = DateTime.fromMillisecondsSinceEpoch(unix * 1000).toLocal();
    return '${t.year}-${t.month.toString().padLeft(2, '0')}-${t.day.toString().padLeft(2, '0')} '
        '${t.hour.toString().padLeft(2, '0')}:${t.minute.toString().padLeft(2, '0')}:${t.second.toString().padLeft(2, '0')}';
  }

  Widget _section(String title, IconData icon, Widget child) {
    return Card(
      elevation: 0,
      margin: const EdgeInsets.only(bottom: 16),
      shape: RoundedRectangleBorder(
        side: BorderSide(
          color: Theme.of(context).dividerColor.withOpacity(0.35),
        ),
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
            const SizedBox(height: 14),
            child,
          ],
        ),
      ),
    );
  }

  Widget _kv(String label, String value, {Widget? trailing}) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 150,
            child: Text(
              label,
              style: TextStyle(
                color: Theme.of(context)
                    .textTheme
                    .bodyMedium
                    ?.color
                    ?.withOpacity(0.58),
              ),
            ),
          ),
          Expanded(child: SelectableText(value.isEmpty ? '--' : value)),
          if (trailing != null) trailing,
        ],
      ),
    );
  }

  Widget _networkSection() {
    if (_loadingNetwork && _interfaces.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (_networkError.isNotEmpty && _interfaces.isEmpty) {
      return Text('读取网络配置失败：$_networkError');
    }

    return Column(
      children: _interfaces.map((item) {
        final status = item['status']?.toString() ?? '';
        final up = status.toLowerCase() == 'up';
        return Container(
          padding: const EdgeInsets.symmetric(vertical: 10),
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(
                color: Theme.of(context).dividerColor.withOpacity(0.22),
              ),
            ),
          ),
          child: Column(
            children: [
              Row(
                children: [
                  Icon(
                    up ? Icons.lan_rounded : Icons.link_off_rounded,
                    size: 18,
                    color: up
                        ? const Color(0xFF2E9B65)
                        : Theme.of(context).disabledColor,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      item['alias']?.toString() ?? '--',
                      style: const TextStyle(fontWeight: FontWeight.w600),
                    ),
                  ),
                  Text(item['type']?.toString() ?? 'Other'),
                  const SizedBox(width: 12),
                  Text(up ? 'Up' : (status.isEmpty ? '--' : status)),
                ],
              ),
              const SizedBox(height: 5),
              _kv('IPv4', item['ipv4']?.toString() ?? ''),
              _kv('MAC', item['mac']?.toString() ?? ''),
              _kv('网关', item['gateway']?.toString() ?? ''),
              _kv('DNS', item['dns']?.toString() ?? ''),
              _kv('适配器', item['description']?.toString() ?? ''),
            ],
          ),
        );
      }).toList(),
    );
  }

  String _diagnosticSummary() {
    if (_probes.isEmpty) return _runningDiagnostics ? '诊断中' : '未诊断';
    for (final probe in _probes) {
      if (probe.ok) continue;
      if (probe.name != 'Direct' && probe.address.isEmpty) return 'DNS异常';
      if (probe.name == 'ID Server') return 'ID Server异常';
      if (probe.name == 'Relay') return 'Relay异常';
      if (probe.name == 'API') return 'API异常';
      if (probe.name == 'Direct') return 'Direct异常';
    }
    return gFFI.serverModel.connectStatus == 1 ? '正常' : 'Server连接异常';
  }

  Widget _diagnosticSection() {
    final serverConnected = gFFI.serverModel.connectStatus == 1;
    final summary = _diagnosticSummary();
    final healthy = summary == '正常';
    return Column(
      children: [
        _kv(
          '诊断结果',
          summary,
          trailing: Icon(
            healthy ? Icons.check_circle : Icons.info_outline,
            size: 18,
            color: healthy
                ? const Color(0xFF2E9B65)
                : const Color(0xFFD9A441),
          ),
        ),
        _kv(
          'RustDesk Server',
          serverConnected ? '已连接' : '未连接 / 连接中',
          trailing: Icon(
            Icons.circle,
            size: 10,
            color: serverConnected
                ? const Color(0xFF2E9B65)
                : const Color(0xFFD9A441),
          ),
        ),
        ..._probes.map((probe) {
          final detail = probe.ok
              ? '${probe.host}:${probe.port} · ${probe.address} · ${probe.elapsedMs} ms'
              : '${probe.host}:${probe.port} · ${probe.error}';
          return _kv(
            probe.name,
            detail,
            trailing: Icon(
              probe.ok ? Icons.check_circle : Icons.error_outline,
              size: 18,
              color: probe.ok
                  ? const Color(0xFF2E9B65)
                  : const Color(0xFFD9534F),
            ),
          );
        }),
        if (_diagnosticTime != null)
          _kv(
            '诊断时间',
            _formatUnix(_diagnosticTime!.millisecondsSinceEpoch ~/ 1000),
          ),
      ],
    );
  }

  Widget _serverSection() {
    return Column(
      children: [
        _kv(
          'ID Server',
          '${_stringConfig('rendezvous', '--')}:${_intConfig('rendezvous_port', 21116)}',
        ),
        _kv(
          'Relay Server',
          '${_stringConfig('relay', '--')}:${_intConfig('relay_port', 21117)}',
        ),
        _kv(
          'API Server',
          '${_stringConfig('api', '--')}:${_intConfig('api_port', 443)}',
        ),
        _kv('Direct Port', _intConfig('direct_port', 21118).toString()),
        _kv(
          'LAN Discovery',
          _serverConfig['lan_discovery_reply'] == false
              ? '单向发现'
              : '双向发现（Cat 设备可互相发现）',
        ),
      ],
    );
  }

  Widget _updateSection() {
    final currentBuild =
        (_updateState['current_build'] as num?)?.toInt() ?? 0;
    final availableBuild =
        (_updateState['available_build'] as num?)?.toInt();
    final hasUpdate =
        availableBuild != null && availableBuild > currentBuild;

    return Column(
      children: [
        _kv(
          '当前版本',
          '${_updateState['current_version'] ?? '--'} / Build $currentBuild',
        ),
        _kv(
          'Stable 最新',
          _updateState['available_version'] == null
              ? '--'
              : '${_updateState['available_version']} / Build ${availableBuild ?? '--'}',
        ),
        _kv('更新通道', _updateState['channel']?.toString() ?? 'stable'),
        _kv('上次检查', _formatUnix(_updateState['last_check_unix'])),
        _kv(
          '状态',
          hasUpdate
              ? '发现新版本'
              : _updateResultText(
                  _updateState['last_result']?.toString() ?? '',
                ),
        ),
        _kv(
          '下载状态',
          _updateState['downloaded'] == true ? '已下载' : '未下载',
        ),
        _kv('来源', _updateState['source']?.toString() ?? '--'),
        if ((_updateState['last_error']?.toString() ?? '').isNotEmpty)
          _kv('失败原因', _updateState['last_error'].toString()),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(24),
      child: Column(
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  '网络、服务器与更新诊断',
                  style: TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
                ),
              ),
              OutlinedButton.icon(
                onPressed: _loadingNetwork ? null : _loadNetworkInfo,
                icon: const Icon(Icons.lan_outlined, size: 18),
                label: const Text('刷新网络'),
              ),
              const SizedBox(width: 8),
              OutlinedButton.icon(
                onPressed: _runningDiagnostics ? null : _runDiagnostics,
                icon: _runningDiagnostics
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.health_and_safety_outlined, size: 18),
                label: const Text('重新诊断'),
              ),
              const SizedBox(width: 8),
              FilledButton.icon(
                onPressed: _checkingUpdate ? null : _checkUpdate,
                icon: _checkingUpdate
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.system_update_alt_rounded, size: 18),
                label: const Text('检查更新'),
              ),
            ],
          ),
          const SizedBox(height: 16),
          _section('高级网络信息', Icons.lan_outlined, _networkSection()),
          _section('服务器连接详情', Icons.dns_outlined, _serverSection()),
          _section(
            '网络诊断',
            Icons.health_and_safety_outlined,
            _diagnosticSection(),
          ),
          _section('更新详情', Icons.system_update_alt_rounded, _updateSection()),
        ],
      ),
    );
  }
}
