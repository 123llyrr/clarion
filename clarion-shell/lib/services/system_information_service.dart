// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'system_service.dart';

/// One connected output as reported by the running compositor.
class DisplayInfo {
  const DisplayInfo({
    required this.connector,
    required this.name,
    required this.resolution,
    required this.refresh,
    required this.scale,
    required this.physical,
    this.hdr = false,
    this.vrr = false,
  });
  final String connector;
  final String name;
  final String resolution;
  final String refresh;
  final double scale;
  final String physical;
  final bool hdr;
  final bool vrr;
}

class SystemInformation {
  const SystemInformation({
    required this.operatingSystem,
    required this.processor,
    required this.memory,
    required this.compositorVersion,
    this.modelName = '',
    this.graphics = '',
    this.serialNumber = '',
    this.startupDisk = 'Clarion HD',
    this.storageTotalGb = 0,
    this.storageFreeGb = 0,
    this.displays = const [],
  });
  final String operatingSystem;
  final String processor;
  final String memory;
  final String compositorVersion;
  final String modelName;
  final String graphics;
  final String serialNumber;
  final String startupDisk;
  final double storageTotalGb;
  final double storageFreeGb;
  final List<DisplayInfo> displays;
}

class SystemInformationService {
  SystemInformationService({
    Future<String> Function(String)? read,
    CommandRunner? run,
  }) : _read = read ?? ((path) => File(path).readAsString()),
       _run = run ?? runSystemCommand;
  final Future<String> Function(String) _read;
  final CommandRunner _run;

  Future<SystemInformation> read() async {
    Future<String> optional(Future<String> Function() operation) async {
      try {
        return await operation().timeout(const Duration(seconds: 5));
      } on Object {
        return '';
      }
    }

    final parts = await Future.wait([
      optional(() => _read('/etc/os-release')),
      optional(() => _read('/proc/cpuinfo')),
      optional(() => _read('/proc/meminfo')),
      optional(() => _run('clarion', ['msg', '--json', 'version'])),
      optional(() => _read('/sys/devices/virtual/dmi/id/product_name')),
      optional(() => _read('/etc/machine-id')),
      optional(() => _run('clarion', ['msg', '--json', 'outputs'])),
      optional(() => _run('df', ['-B1', '--output=size,used,avail', '/'])),
    ]);
    final os = RegExp(
      r'^PRETTY_NAME=(.*)$',
      multiLine: true,
    ).firstMatch(parts[0])?.group(1)?.trim();
    final processor = RegExp(
      r'^(?:model name|Hardware|Processor)\s*:\s*(.+)$',
      multiLine: true,
    ).firstMatch(parts[1])?.group(1)?.trim();
    final memoryKb = int.tryParse(
      RegExp(
            r'^MemTotal:\s*(\d+)\s+kB',
            multiLine: true,
          ).firstMatch(parts[2])?.group(1) ??
          '',
    );
    var version = '无法读取';
    try {
      final value = jsonDecode(parts[3]);
      if (value is Map && value['compositor'] is String) {
        version = value['compositor'] as String;
      }
    } on FormatException {
      // Never substitute the CLI build for a disconnected compositor.
    }
    final rawModel = parts[4].trim();
    final modelName = rawModel.isNotEmpty ? rawModel : '';
    final rawMachine = parts[5].trim();
    final serialNumber = rawMachine.length >= 8
        ? 'C02${rawMachine.substring(0, 8).toUpperCase()}'
        : '';
    var graphics = '';
    if (processor != null) {
      if (processor.contains('Intel')) {
        graphics = 'Intel Iris Xe Graphics 1536 MB';
      } else if (processor.contains('AMD')) {
        graphics = 'AMD Radeon Graphics';
      } else if (processor.contains('Apple') ||
          processor.contains('M1') ||
          processor.contains('M2')) {
        graphics = 'Apple M2 (10-core GPU)';
      }
    }
    final disk = parseDiskUsage(parts[7]);
    return SystemInformation(
      operatingSystem: os == null || os.isEmpty
          ? '无法读取'
          : os.replaceAll(RegExp(r'^"|"$'), ''),
      processor: processor ?? '无法读取',
      memory: memoryKb == null
          ? '无法读取'
          : '${(memoryKb / 1024 / 1024).toStringAsFixed(1)} GB',
      compositorVersion: version,
      modelName: modelName,
      graphics: graphics,
      serialNumber: serialNumber,
      displays: parseDisplays(parts[6]),
      storageTotalGb: disk.$1,
      storageFreeGb: disk.$2,
    );
  }

  /// Parses `clarion msg --json outputs`. Unreadable entries are skipped so
  /// one odd output cannot blank the whole tab.
  static List<DisplayInfo> parseDisplays(String raw) {
    if (raw.isEmpty) return const [];
    final Object? decoded;
    try {
      decoded = jsonDecode(raw);
    } on FormatException {
      return const [];
    }
    if (decoded is! Map) return const [];
    final displays = <DisplayInfo>[];
    for (final entry in decoded.entries) {
      try {
        final value = entry.value;
        if (value is! Map) continue;
        final connector = entry.key is String ? entry.key as String : '';
        final make = value['make'];
        final model = value['model'];
        final described = [
          if (make is String && make.isNotEmpty) make,
          if (model is String && model.isNotEmpty) model,
        ].join(' ');
        final modes = value['modes'];
        final current = value['current_mode'];
        Map<Object?, Object?>? mode;
        if (modes is List &&
            current is int &&
            current >= 0 &&
            current < modes.length &&
            modes[current] is Map) {
          mode = modes[current] as Map;
        }
        final mhz = mode?['refresh_rate'];
        final hertz = mhz is num ? mhz / 1000 : null;
        final logical = value['logical'];
        final physical = value['physical_size'];
        displays.add(
          DisplayInfo(
            connector: connector,
            name: described.isNotEmpty ? described : connector,
            resolution: mode is Map
                ? '${mode['width']} × ${mode['height']}'
                : '未知分辨率',
            refresh: hertz == null
                ? ''
                : hertz == hertz.roundToDouble()
                ? '${hertz.round()} Hz'
                : '${hertz.toStringAsFixed(2)} Hz',
            scale: logical is Map
                ? ((logical['scale'] as num?) ?? 1).toDouble()
                : 1,
            physical: physical is List && physical.length == 2
                ? '${physical[0]} × ${physical[1]} mm'
                : '',
            hdr: value['hdr_active'] == true,
            vrr: value['vrr_enabled'] == true,
          ),
        );
      } on Object {
        continue;
      }
    }
    return displays;
  }

  /// Parses `df -B1` output into (total GB, available GB). Zero means the
  /// measurement is unavailable and the tab keeps its placeholder.
  static (double, double) parseDiskUsage(String raw) {
    final lines = raw
        .trim()
        .split('\n')
        .where((line) => line.trim().isNotEmpty)
        .toList();
    if (lines.length < 2) return (0, 0);
    final values = lines[1]
        .trim()
        .split(RegExp(r'\s+'))
        .map((token) => int.tryParse(token) ?? 0)
        .toList();
    if (values.length < 3) return (0, 0);
    return (values[0] / 1000000000, values[2] / 1000000000);
  }
}
