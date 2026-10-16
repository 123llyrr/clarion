// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:io';

import 'user_directories.dart';

/// A named session is authoritative; a failed connection must not select another desktop.
String? ipcSocketPath(Map<String, String> environment) {
  final canonical = environment['CLARION_SOCKET'];
  if (canonical != null && canonical.isNotEmpty) return canonical;
  final legacy = environment['REALAND_SOCKET'];
  return legacy != null && legacy.isNotEmpty ? legacy : null;
}

/// Retain existing preferences, Dock pins and per-output desktop positions without copying files.
/// New installations use Clarion; creating its directory explicitly selects the new namespace.
String shellConfigDirectory(Map<String, String> environment) {
  final root = userConfigDirectory(environment);
  final canonical = '$root/clarion-shell';
  final legacy = '$root/realand-shell';
  try {
    if (FileSystemEntity.typeSync(canonical, followLinks: false) ==
            FileSystemEntityType.notFound &&
        FileSystemEntity.typeSync(legacy, followLinks: false) !=
            FileSystemEntityType.notFound) {
      return legacy;
    }
  } on FileSystemException {
    // Keep an inaccessible canonical path authoritative; individual stores report/load errors.
  }
  return canonical;
}

File? shellConfigurationFile(String name, {Map<String, String>? environment}) {
  try {
    return File(
      '${shellConfigDirectory(environment ?? Platform.environment)}/$name',
    );
  } on StateError {
    return null;
  }
}
