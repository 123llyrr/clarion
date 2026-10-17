// SPDX-License-Identifier: GPL-3.0-or-later

/// An empty locale variable is unset, so it must not mask a lower-priority
/// locale. Desktop Entry localized keys omit the locale's encoding suffix.
String desktopEntryLocale(Map<String, String> environment) =>
    [environment['LC_ALL'], environment['LC_MESSAGES'], environment['LANG']]
        .whereType<String>()
        .firstWhere((value) => value.isNotEmpty, orElse: () => '')
        .split('.')
        .first;
