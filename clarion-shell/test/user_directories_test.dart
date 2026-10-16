// SPDX-License-Identifier: GPL-3.0-or-later
import 'package:flutter_test/flutter_test.dart';
import 'package:clarion_shell/services/user_directories.dart';

void main() {
  test(
    'absolute user and XDG directories take priority without filesystem access',
    () {
      const environment = {
        'HOME': '/fixture/home',
        'XDG_CONFIG_HOME': '/fixture/config',
        'XDG_CACHE_HOME': '/fixture/cache',
      };
      expect(userHomeDirectory(environment), '/fixture/home');
      expect(userConfigDirectory(environment), '/fixture/config');
      expect(userCacheDirectory(environment), '/fixture/cache');
    },
  );

  test('relative XDG directories safely fall back to the supplied home', () {
    const environment = {
      'HOME': '/fixture/home',
      'XDG_CONFIG_HOME': 'relative',
      'XDG_CACHE_HOME': '',
    };
    expect(userConfigDirectory(environment), '/fixture/home/.config');
    expect(userCacheDirectory(environment), '/fixture/home/.cache');
  });

  test('missing or relative HOME rejects unsafe implicit file locations', () {
    expect(() => userHomeDirectory({}), throwsStateError);
    expect(() => userHomeDirectory({'HOME': 'relative'}), throwsStateError);
    expect(() => userConfigDirectory({}), throwsStateError);
    expect(() => userCacheDirectory({}), throwsStateError);
    expect(
      userConfigDirectory({'XDG_CONFIG_HOME': '/fixture/config'}),
      '/fixture/config',
    );
    expect(
      userCacheDirectory({'XDG_CACHE_HOME': '/fixture/cache'}),
      '/fixture/cache',
    );
  });
}
