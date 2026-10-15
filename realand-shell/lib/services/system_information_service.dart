// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:convert';
import 'dart:io';

import 'system_service.dart';

class SystemInformation {
  const SystemInformation({
    required this.operatingSystem,
    required this.processor,
    required this.memory,
    required this.compositorVersion,
    this.modelName = '',
    this.graphics = '',
    this.serialNumber = '',
    this.startupDisk = 'Macintosh HD',
    this.storageTotalGb = 450,
    this.storageFreeGb = 112,
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
      optional(() => _run('realand', ['msg', '--json', 'version'])),
      optional(() => _read('/sys/devices/virtual/dmi/id/product_name')),
      optional(() => _read('/etc/machine-id')),
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
      } else if (processor.contains('Apple') || processor.contains('M1') || processor.contains('M2')) {
        graphics = 'Apple M2 (10-core GPU)';
      }
    }
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
    );
  }
}
