// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'dart:convert';
import 'dart:io';

typedef ApplicationCommandRunner =
    Future<String> Function(String executable, List<String> arguments);
typedef ApplicationScopeProbe = Future<ApplicationScope?> Function();

/// CLI capabilities belong to systemd-run, not to the manager's version.
class ApplicationScope {
  const ApplicationScope({this.expandEnvironment = true, this.collect = true});

  final bool expandEnvironment;
  final bool collect;

  factory ApplicationScope.fromSystemdHelp(String help, String version) {
    final release = int.tryParse(
      RegExp(
            r'^systemd\s+(\d+)',
            multiLine: true,
          ).firstMatch(version)?.group(1) ??
          '',
    );
    if (release == null ||
        !help.contains('--scope') ||
        !help.contains('--user') ||
        !help.contains('--slice=')) {
      throw StateError('systemd-run 不支持独立应用 scope');
    }
    final expansion = help.contains('--expand-environment=');
    // Before v254 scope executes argv directly, without variable expansion.
    // Newer releases must accept the explicit switch; never guess its default.
    if (release >= 254 && !expansion) {
      throw StateError('systemd-run 无法保证应用参数保持原样');
    }
    return ApplicationScope(
      expandEnvironment: expansion,
      collect: help.contains('--collect'),
    );
  }

  List<String> arguments(String executable, List<String> arguments) => [
    '--user',
    '--scope',
    '--quiet',
    if (collect) '--collect',
    '--slice=app.slice',
    if (expandEnvironment) '--expand-environment=no',
    '--',
    executable,
    ...arguments,
  ];
}

/// Separates user applications from the desktop shell's service lifetime.
/// GIO still owns Desktop Entry expansion, MIME handling and D-Bus activation.
class ApplicationLauncher {
  ApplicationLauncher({
    Map<String, String>? environment,
    ApplicationCommandRunner? command,
    ApplicationScopeProbe? scopeProbe,
  }) : environment = Map.unmodifiable({
         ...Platform.environment,
         ...?environment,
       }),
       _injectedCommand = command,
       _injectedProbe = scopeProbe;

  final Map<String, String> environment;
  final ApplicationCommandRunner? _injectedCommand;
  final ApplicationScopeProbe? _injectedProbe;
  Future<ApplicationScope>? _capabilities;

  /// Wait for short-lived helpers such as GIO to report their exit status.
  /// Direct GUI applications use [waitForExit] false and retain their process
  /// after the initial startup-error observation period.
  Future<void> launch(
    String executable,
    List<String> arguments, {
    bool waitForExit = true,
  }) async {
    // Probe before invoking the application. Once an invocation is attempted,
    // an error must reach the caller; retrying directly could open it twice.
    final scope = await (_injectedProbe?.call() ?? _scope());
    final command =
        _injectedCommand ??
        (String program, List<String> argv) =>
            _run(program, argv, waitForExit: waitForExit);
    if (scope == null) {
      await command(executable, List.of(arguments));
    } else {
      await command('systemd-run', scope.arguments(executable, arguments));
    }
  }

  Future<ApplicationScope?> _scope() async {
    if (!Platform.isLinux) return null;
    final runtime = environment['XDG_RUNTIME_DIR'];
    if (runtime == null || !runtime.startsWith('/')) return null;
    // This is the same user-manager socket that systemctl --user connects to.
    // An absent manager permits a direct launch. Permission, timeout or other
    // failures with an existing manager must not silently bypass isolation.
    final socket = await FileSystemEntity.type('$runtime/systemd/private');
    if (socket == FileSystemEntityType.notFound) return null;
    await _probe('systemctl', [
      '--user',
      'show',
      '--property=Version',
      '--value',
    ]);
    try {
      return await (_capabilities ??= _readCapabilities());
    } on Object {
      _capabilities = null;
      rethrow;
    }
  }

  Future<ApplicationScope> _readCapabilities() async {
    final help = await _probe('systemd-run', ['--help']);
    final version = await _probe('systemd-run', ['--version']);
    return ApplicationScope.fromSystemdHelp(help, version);
  }

  Future<String> _probe(String executable, List<String> arguments) async {
    final process = await Process.start(
      executable,
      arguments,
      environment: environment,
    );
    final output = _boundedOutput(process.stdout);
    final error = _boundedOutput(process.stderr);
    try {
      final exit = await process.exitCode.timeout(const Duration(seconds: 3));
      final stdout = await output;
      final stderr = await error;
      if (exit != 0) {
        throw StateError(
          stderr.trim().isEmpty ? '$executable failed ($exit)' : stderr.trim(),
        );
      }
      return stdout;
    } on TimeoutException {
      process.kill(ProcessSignal.sigkill);
      await process.exitCode;
      rethrow;
    }
  }

  Future<String> _run(
    String executable,
    List<String> arguments, {
    required bool waitForExit,
  }) async {
    // Apps inherit the shell's journal descriptors, rather than temporary
    // command pipes which would close when GIO returns or the shell exits.
    // inheritStdio still exposes exitCode, preserving GIO/scope launch errors.
    final process = await Process.start(
      executable,
      arguments,
      environment: environment,
      mode: ProcessStartMode.inheritStdio,
    );
    try {
      final exit = await process.exitCode.timeout(
        Duration(seconds: waitForExit ? 10 : 1),
      );
      if (exit != 0) {
        throw StateError('应用启动失败：$executable ($exit)，详见外壳日志');
      }
      return '';
    } on TimeoutException {
      // A foreground GUI remains alive until its window closes. Observe early
      // startup failures, then leave its lifetime to the application/user
      // manager; a running window is not a failed launch helper.
      if (!waitForExit) return '';
      // Stop only the launch helper. Any application already placed in its
      // scope remains owned by the user manager, independent of this shell.
      process.kill(ProcessSignal.sigkill);
      rethrow;
    }
  }
}

Future<String> _boundedOutput(Stream<List<int>> stream) async {
  final bytes = <int>[];
  await for (final chunk in stream) {
    final remaining = 64 * 1024 - bytes.length;
    if (remaining > 0) bytes.addAll(chunk.take(remaining));
  }
  return utf8.decode(bytes, allowMalformed: true);
}
