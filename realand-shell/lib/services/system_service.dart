// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import '../models.dart';
import 'application_launcher.dart';

typedef CommandRunner =
    Future<String> Function(String executable, List<String> arguments);

double? _finiteDouble(String value) {
  final number = double.tryParse(value);
  return number != null && number.isFinite ? number : null;
}

Future<String> runSystemCommand(
  String executable,
  List<String> arguments, {
  Map<String, String>? environment,
}) async {
  final process = await Process.start(
    executable,
    arguments,
    environment: environment,
  );
  final output = _CommandPipe(process.stdout);
  final error = _CommandPipe(process.stderr);
  try {
    final code = await process.exitCode.timeout(const Duration(seconds: 5));
    final results = await Future.wait([output.finish(), error.finish()]);
    if (code != 0) {
      throw StateError(
        results[1].trim().isEmpty
            ? '$executable failed ($code)'
            : results[1].trim(),
      );
    }
    return results[0].trim();
  } on TimeoutException {
    process.kill(ProcessSignal.sigkill);
    rethrow;
  } finally {
    await Future.wait([output.cancel(), error.cancel()]);
  }
}

class _CommandPipe {
  _CommandPipe(Stream<List<int>> source) {
    _subscription = source.listen(
      (chunk) {
        // Command feedback is bounded even if an external tool is noisy.
        final remaining = 256 * 1024 - _bytes.length;
        if (remaining > 0) _bytes.add(chunk.take(remaining).toList());
      },
      onDone: () {
        if (!_done.isCompleted) _done.complete();
      },
      onError: (Object error) {
        if (!_done.isCompleted) _done.complete();
      },
    );
  }
  final _bytes = BytesBuilder(copy: false);
  final _done = Completer<void>();
  late final StreamSubscription<List<int>> _subscription;
  Future<String> finish() async {
    await _done.future.timeout(
      const Duration(milliseconds: 500),
      onTimeout: () {},
    );
    return utf8.decode(_bytes.takeBytes(), allowMalformed: true);
  }

  Future<void> cancel() => _subscription.cancel();
}

class SystemSnapshot {
  const SystemSnapshot({
    this.volume,
    this.brightness,
    this.batteryFraction,
    this.muted = false,
    this.charging = false,
    this.wifiEnabled,
    this.bluetoothEnabled,
    this.networkName = '',
    this.isWireless = false,
  });
  final double? volume;
  final double? brightness;
  final double? batteryFraction;
  final bool muted;
  final bool charging;
  final bool? wifiEnabled;
  final bool? bluetoothEnabled;
  final String networkName;
  final bool isWireless;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is SystemSnapshot &&
          volume == other.volume &&
          brightness == other.brightness &&
          batteryFraction == other.batteryFraction &&
          muted == other.muted &&
          charging == other.charging &&
          wifiEnabled == other.wifiEnabled &&
          bluetoothEnabled == other.bluetoothEnabled &&
          networkName == other.networkName &&
          isWireless == other.isWireless;

  @override
  int get hashCode => Object.hash(
    volume,
    brightness,
    batteryFraction,
    muted,
    charging,
    wifiEnabled,
    bluetoothEnabled,
    networkName,
    isWireless,
  );
}

class SystemAudioSnapshot {
  const SystemAudioSnapshot({required this.volume, required this.muted});
  final double? volume;
  final bool muted;

  factory SystemAudioSnapshot.parse(String source) => SystemAudioSnapshot(
    volume: _finiteDouble(
      RegExp(r'Volume:\s*([0-9.]+)').firstMatch(source)?.group(1) ?? '',
    )?.clamp(0.0, 1.0),
    muted: source.contains('[MUTED]'),
  );
}

class SystemService {
  SystemService({
    CommandRunner? command,
    Map<String, String>? environment,
    ApplicationLauncher? applicationLauncher,
    this.powerSupplyRoot = '/sys/class/power_supply',
  }) : command = command ?? runSystemCommand,
       environment = environment ?? Platform.environment,
       _applicationLauncher =
           applicationLauncher ?? ApplicationLauncher(environment: environment);
  final CommandRunner command;
  final Map<String, String> environment;
  final String powerSupplyRoot;
  final ApplicationLauncher _applicationLauncher;

  /// GIO resolves the user's GUI file manager. Locations stay literal argv,
  /// including homes with spaces or shell metacharacters.
  Future<void> openHomeFolder() async {
    final home = environment['HOME'];
    if (home == null || !home.startsWith('/')) {
      throw StateError('用户主目录不可用');
    }
    await _applicationLauncher.launch('gio', ['open', '--', home]);
  }

  Future<void> openTrash() async {
    await _applicationLauncher.launch('gio', ['open', '--', 'trash:///']);
  }

  Future<void> emptyTrash() async {
    await _applicationLauncher.launch('gio', ['trash', '--empty']);
  }

  Future<void> openLocation(String location) async {
    if (!location.startsWith('/') || location.contains('\u0000')) {
      throw ArgumentError.value(location, 'location', 'Absolute path required');
    }
    await _applicationLauncher.launch('gio', ['open', '--', location]);
  }

  Future<void> openDownloadsFolder() async {
    final home = environment['HOME'];
    if (home == null || !home.startsWith('/')) {
      throw StateError('用户下载目录不可用');
    }
    var location = '$home/Downloads';
    try {
      final configured = (await command('xdg-user-dir', ['DOWNLOAD'])).trim();
      if (configured.startsWith('/') && !configured.contains('\u0000')) {
        location = configured;
      }
    } on Object {
      // xdg-user-dirs is optional. Resolve a literal default without a shell.
    }
    await _applicationLauncher.launch('gio', ['open', '--', location]);
  }

  Future<void> openDesktopFolder() async {
    final home = environment['HOME'];
    if (home == null || !home.startsWith('/')) {
      throw StateError('用户桌面目录不可用');
    }
    var location = '$home/Desktop';
    try {
      final configured = (await command('xdg-user-dir', ['DESKTOP'])).trim();
      if (configured.startsWith('/') && !configured.contains('\u0000')) {
        location = configured;
      }
    } on Object {
      if (Directory('$home/桌面').existsSync()) {
        location = '$home/桌面';
      }
    }
    await _applicationLauncher.launch('gio', ['open', '--', location]);
  }

  Future<SystemSnapshot> read() async {
    Future<String?> optional(String program, List<String> args) async {
      try {
        return await command(program, args);
      } on Object {
        return null;
      }
    }

    final values = await Future.wait([
      optional('wpctl', ['get-volume', '@DEFAULT_AUDIO_SINK@']),
      optional('brightnessctl', ['--class=backlight', '-m']),
      optional('nmcli', ['radio', 'wifi']),
      optional('bluetoothctl', ['show']),
      optional('nmcli', [
        '-t',
        '-f',
        'NAME,TYPE',
        'connection',
        'show',
        '--active',
      ]),
    ]);
    final audio = values[0];
    final volume = audio == null
        ? null
        : _finiteDouble(
            RegExp(r'Volume:\s*([0-9.]+)').firstMatch(audio)?.group(1) ?? '',
          )?.clamp(0.0, 1.0);
    double? brightness;
    if (values[1] case final text?) {
      for (final line in text.split('\n')) {
        final fields = line.split(',');
        if (fields.length < 5) continue;
        final current = _finiteDouble(fields[2]);
        final maximum = _finiteDouble(fields[4]);
        if (current != null && maximum != null && maximum > 0) {
          brightness = (current / maximum).clamp(0.0, 1.0);
          break;
        }
      }
    }
    final battery = await readBattery(powerSupplyRoot);
    final bluetooth = values[3];
    final powered = bluetooth == null
        ? null
        : RegExp(r'Powered:\s*(yes|no)').firstMatch(bluetooth)?.group(1);
      final network = activeNetworkDetails(values[4] ?? '');
      return SystemSnapshot(
        volume: volume,
        muted: audio?.contains('[MUTED]') ?? false,
        brightness: brightness,
        batteryFraction: battery.$1,
        charging: battery.$2,
        wifiEnabled: switch (values[2]) {
          'enabled' => true,
          'disabled' => false,
          _ => null,
        },
        bluetoothEnabled: switch (powered) {
          'yes' => true,
          'no' => false,
          _ => null,
        },
        networkName: network.$1,
        isWireless: network.$2,
      );
  }

  Future<void> setVolume(double value) async {
    if (!value.isFinite) {
      throw ArgumentError.value(value, 'value', 'Volume must be finite');
    }
    await command('wpctl', [
      'set-volume',
      '@DEFAULT_AUDIO_SINK@',
      value.clamp(0.0, 1.0).toStringAsFixed(3),
      '--limit',
      '1.0',
    ]);
  }

  Future<SystemAudioSnapshot> readAudio() async => SystemAudioSnapshot.parse(
    await command('wpctl', ['get-volume', '@DEFAULT_AUDIO_SINK@']),
  );

  Future<void> openSoundPreferences() =>
      _applicationLauncher.launch('pavucontrol', const []);

  Future<void> toggleMute() async {
    await command('wpctl', ['set-mute', '@DEFAULT_AUDIO_SINK@', 'toggle']);
  }

  Future<void> setBrightness(double value) async {
    if (!value.isFinite) {
      throw ArgumentError.value(value, 'value', 'Brightness must be finite');
    }
    // Keep a visible screen; changing DPMS is a distinct user action.
    await command('brightnessctl', [
      '--class=backlight',
      'set',
      '${(value.clamp(0.01, 1.0) * 100).round()}%',
    ]);
  }

  Future<void> setWifi(bool enabled) async {
    await command('nmcli', ['radio', 'wifi', enabled ? 'on' : 'off']);
  }

  Future<void> setBluetooth(bool enabled) async {
    await command('bluetoothctl', ['power', enabled ? 'on' : 'off']);
  }

  Future<List<WifiNetwork>> scanWifiNetworks() async {
    try {
      final output = await command('nmcli', [
        '-t',
        '-f',
        'IN-USE,SSID,SIGNAL,SECURITY',
        'device',
        'wifi',
        'list',
      ]);
      return parseWifiNetworks(output);
    } catch (_) {
      return const [];
    }
  }

  Future<void> connectWifi(String ssid, {String? password}) async {
    final args = <String>['device', 'wifi', 'connect', ssid];
    if (password != null && password.isNotEmpty) {
      args.addAll(['password', password]);
    }
    await command('nmcli', args);
  }

  Future<List<BluetoothDevice>> listBluetoothDevices() async {
    try {
      final output = await command('bluetoothctl', ['devices']);
      final devices = <BluetoothDevice>[];
      for (final rawLine in output.split('\n')) {
        final line = rawLine.trim();
        if (!line.startsWith('Device ')) continue;
        final parts = line.split(' ');
        if (parts.length < 3) continue;
        final mac = parts[1];
        final name = parts.sublist(2).join(' ');
        var connected = false;
        var icon = 'bluetooth';
        try {
          final info = await command('bluetoothctl', ['info', mac]);
          connected = info.contains('Connected: yes');
          for (final infoLine in info.split('\n')) {
            final trimmed = infoLine.trim();
            if (trimmed.startsWith('Icon:')) {
              icon = trimmed.substring(5).trim();
            }
          }
        } catch (_) {}
        devices.add(BluetoothDevice(
          mac: mac,
          name: name,
          connected: connected,
          icon: icon,
        ));
      }
      devices.sort((a, b) {
        if (a.connected != b.connected) return a.connected ? -1 : 1;
        return a.name.compareTo(b.name);
      });
      return devices;
    } catch (_) {
      return const [];
    }
  }

  Future<void> connectBluetooth(String mac) async {
    await command('bluetoothctl', ['connect', mac]);
  }

  Future<void> disconnectBluetooth(String mac) async {
    await command('bluetoothctl', ['disconnect', mac]);
  }

  Future<void> openNetworkPreferences() async {
    try {
      await _applicationLauncher.launch('nm-connection-editor', const []);
    } catch (_) {
      await _applicationLauncher.launch('realand-settings', const []);
    }
  }

  Future<void> openBluetoothPreferences() async {
    try {
      await _applicationLauncher.launch('blueman-manager', const []);
    } catch (_) {
      await _applicationLauncher.launch('realand-settings', const []);
    }
  }
}

(String, bool) activeNetworkDetails(String source) {
  for (final line in source.split('\n')) {
    final name = StringBuffer();
    for (var index = 0; index < line.length; index++) {
      final character = line[index];
      if (character == r'\' && index + 1 < line.length) {
        final next = line[index + 1];
        if (next == ':' || next == r'\') {
          name.write(next);
          index++;
          continue;
        }
      }
      if (character != ':') {
        name.write(character);
        continue;
      }
      final type = line.substring(index + 1);
      final isWifi = type == '802-11-wireless' || type == 'wifi';
      if (isWifi ||
          type == '802-3-ethernet' ||
          type == 'ethernet') {
        return (name.toString(), isWifi);
      }
      break;
    }
  }
  return ('', false);
}

String activeNetworkName(String source) => activeNetworkDetails(source).$1;

List<WifiNetwork> parseWifiNetworks(String source) {
  final networks = <String, WifiNetwork>{};
  for (final rawLine in source.split('\n')) {
    final line = rawLine.trim();
    if (line.isEmpty) continue;
    final parts = <String>[];
    final sb = StringBuffer();
    for (var i = 0; i < line.length; i++) {
      if (line[i] == r'\' && i + 1 < line.length) {
        sb.write(line[i + 1]);
        i++;
      } else if (line[i] == ':') {
        parts.add(sb.toString());
        sb.clear();
      } else {
        sb.write(line[i]);
      }
    }
    parts.add(sb.toString());
    if (parts.length < 4) continue;
    final inUse = parts[0].trim() == '*';
    final ssid = parts[1].trim();
    if (ssid.isEmpty) continue;
    final signal = int.tryParse(parts[2].trim()) ?? 0;
    final security = parts[3].trim();
    final network = WifiNetwork(
      ssid: ssid,
      signal: signal,
      security: security,
      inUse: inUse,
    );
    final existing = networks[ssid];
    if (existing == null || inUse || (!existing.inUse && signal > existing.signal)) {
      networks[ssid] = network;
    }
  }
  final list = networks.values.toList();
  list.sort((a, b) {
    if (a.inUse != b.inUse) return a.inUse ? -1 : 1;
    return b.signal.compareTo(a.signal);
  });
  return list;
}

Future<(double?, bool)> readBattery(String root) async {
  final capacities = <double>[];
  var charging = false;
  try {
    await for (final device in Directory(root).list(followLinks: true)) {
      Future<String> field(String name) =>
          File('${device.path}/$name').readAsString();
      try {
        if ((await field('type')).trim() != 'Battery') continue;
        final scope = File('${device.path}/scope');
        if (await scope.exists() &&
            (await scope.readAsString()).trim() == 'Device') {
          continue;
        }
        final present = File('${device.path}/present');
        if (await present.exists() &&
            (await present.readAsString()).trim() == '0') {
          continue;
        }
        final capacity = _finiteDouble((await field('capacity')).trim());
        if (capacity != null) capacities.add((capacity / 100).clamp(0.0, 1.0));
        final status = (await field('status')).trim();
        charging |= status == 'Charging' || status == 'Full';
      } on FileSystemException {
        continue;
      }
    }
  } on FileSystemException {
    return (null, false);
  }
  return (
    capacities.isEmpty
        ? null
        : capacities.reduce((a, b) => a + b) / capacities.length,
    charging,
  );
}
