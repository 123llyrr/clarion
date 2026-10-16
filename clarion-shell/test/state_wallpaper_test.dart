// SPDX-License-Identifier: GPL-3.0-or-later
import 'dart:async';
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/models.dart';
import 'package:clarion_shell/services/desktop_apps.dart';
import 'package:clarion_shell/services/clarion_ipc.dart';
import 'package:clarion_shell/services/system_service.dart';
import 'package:clarion_shell/services/wallpaper_service.dart';
import 'package:clarion_shell/state/shell_controller.dart';

const image = ShellWallpaper(
  path: '/fixture/图片.png',
  name: '图片.png',
  sizeBytes: 42,
);
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  test(
    'demo never scans, restores, launches providers or persists wallpapers',
    () async {
      final service = _Wallpapers();
      final controller = ShellController(demo: true, wallpaperService: service);
      addTearDown(controller.dispose);
      await controller.start();
      controller.openWallpapers();
      await controller.refreshWallpapers();
      await controller.applyWallpaper(image);
      expect(controller.panel, ShellPanel.wallpapers);
      expect(controller.selectedWallpaperPath, image.path);
      expect(service.operations, isEmpty);
    },
  );
  test(
    'catalogue is lazy and preview reads selection without restoring a provider',
    () async {
      final service = _Wallpapers();
      final controller = _controller(service, restore: false);
      addTearDown(controller.dispose);
      await controller.start();
      expect(service.operations, ['restore:false']);
      expect(controller.selectedWallpaperPath, '/fixture/saved.png');
      expect(controller.wallpapers, isEmpty);
      controller.openWallpapers();
      await service.scanned.future;
      await Future<void>.delayed(Duration.zero);
      expect(controller.panel, ShellPanel.wallpapers);
      expect(controller.wallpapers.single, image);
      expect(controller.wallpapersLoading, false);
    },
  );
  test(
    'startup restore is nonblocking and explicit apply shows busy and failure without losing selection',
    () async {
      final service = _Wallpapers()..restoreWait = Completer<void>();
      final controller = _controller(service);
      addTearDown(controller.dispose);
      await controller.start();
      expect(controller.busy, contains('wallpaper'));
      expect(service.operations, ['restore:true']);
      service.restoreWait!.complete();
      await Future<void>.delayed(Duration.zero);
      expect(controller.busy, isEmpty);
      service.applyWait = Completer<void>();
      final apply = controller.applyWallpaper(image);
      expect(controller.busy, contains('wallpaper'));
      await controller.applyWallpaper(image);
      expect(service.operations.where((op) => op == 'apply').length, 1);
      service.applyWait!.completeError(StateError('decode failed'));
      await apply;
      expect(controller.wallpaperError, contains('decode failed'));
      expect(controller.selectedWallpaperPath, '/fixture/saved.png');
      expect(controller.busy, isEmpty);
      service.applyWait = null;
      await controller.applyWallpaper(image);
      expect(controller.selectedWallpaperPath, image.path);
      expect(controller.wallpaperError, null);
    },
  );
  test(
    'scan and restore errors are visible and a later refresh recovers',
    () async {
      final service = _Wallpapers()..fail = true;
      final controller = _controller(service);
      addTearDown(controller.dispose);
      await controller.start();
      await Future<void>.delayed(Duration.zero);
      expect(controller.wallpaperError, contains('恢复失败'));
      await controller.refreshWallpapers();
      expect(controller.wallpaperError, contains('列表加载失败'));
      expect(controller.wallpapersLoading, false);
      service.fail = false;
      await controller.refreshWallpapers();
      expect(controller.wallpapers.single, image);
      expect(controller.wallpaperError, null);
    },
  );
}

ShellController _controller(_Wallpapers service, {bool restore = true}) =>
    ShellController(
      notifications: false,
      tray: false,
      restoreWallpaperOnStart: restore,
      wallpaperService: service,
      ipc: _Ipc(),
      system: _System(),
      desktopApps: _Apps(),
    );

class _Wallpapers extends WallpaperService {
  final operations = <String>[];
  final scanned = Completer<void>();
  Completer<void>? restoreWait;
  Completer<void>? applyWait;
  bool fail = false;
  @override
  bool get available => true;
  @override
  Future<void> restore({bool applySaved = false}) async {
    operations.add('restore:$applySaved');
    if (fail) throw StateError('missing');
    await restoreWait?.future;
    selectedPath = '/fixture/saved.png';
  }

  @override
  Future<List<ShellWallpaper>> scan() async {
    operations.add('scan');
    if (fail) throw StateError('unreadable');
    if (!scanned.isCompleted) scanned.complete();
    return const [image];
  }

  @override
  Future<void> apply(ShellWallpaper entry, {bool persist = true}) async {
    operations.add('apply');
    await applyWait?.future;
    selectedPath = entry.path;
  }
}

class _Ipc extends ClarionIpc {
  @override
  Stream<Map<String, dynamic>> get events => const Stream.empty();
  @override
  Stream<bool> get connections => const Stream.empty();
  @override
  void start() {}
}

class _System extends SystemService {
  @override
  Future<SystemSnapshot> read() async => const SystemSnapshot();
}

class _Apps extends DesktopApps {
  @override
  Future<List<DesktopApp>> load() async => [];
}
