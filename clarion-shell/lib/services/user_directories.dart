// SPDX-License-Identifier: GPL-3.0-or-later
/// Resolve only explicit user directories; never guess another user's home.
String userHomeDirectory(Map<String, String> environment) {
  final home = environment['HOME'];
  if (home != null && home.startsWith('/')) return home;
  throw StateError('An absolute HOME directory is required for user files');
}

String userConfigDirectory(Map<String, String> environment) {
  final configured = environment['XDG_CONFIG_HOME'];
  return configured != null && configured.startsWith('/')
      ? configured
      : '${userHomeDirectory(environment)}/.config';
}

String userCacheDirectory(Map<String, String> environment) {
  final configured = environment['XDG_CACHE_HOME'];
  return configured != null && configured.startsWith('/')
      ? configured
      : '${userHomeDirectory(environment)}/.cache';
}
