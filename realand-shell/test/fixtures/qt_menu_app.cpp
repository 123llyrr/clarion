// SPDX-License-Identifier: GPL-3.0-or-later
// Compile with pkg-config Qt6Widgets Qt6DBus; run only on a private bus and
// nested compositor. This uses Qt's real platform menu exporter, not a mock.
#include <QAction>
#include <QApplication>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QElapsedTimer>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLabel>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QSaveFile>
#include <QThread>
#include <QTimer>

#include <array>
#include <memory>

static bool writeFile(const QString &path, const QByteArray &contents) {
  QSaveFile file(path);
  return file.open(QIODevice::WriteOnly) && file.write(contents) == contents.size()
         && file.commit();
}

static bool differentPrivateValue(const char *key, const char *parentKey) {
  const QByteArray value = qgetenv(key);
  return !value.isEmpty() && qEnvironmentVariableIsSet(parentKey)
         && value != qgetenv(parentKey);
}

int main(int argc, char **argv) {
  const QString resultPath = qEnvironmentVariable("REALAND_MENU_QA_RESULT");
  const QString capturePath = qEnvironmentVariable("REALAND_SHELL_CAPTURE_DIR");
  const QString registrarReadyPath = qEnvironmentVariable("REALAND_MENU_QA_READY");
  if (qgetenv("REALAND_SHELL_ISOLATED_TEST") != "1"
      || !differentPrivateValue("WAYLAND_DISPLAY", "REALAND_SHELL_PARENT_WAYLAND_DISPLAY")
      || !differentPrivateValue("REALAND_SOCKET", "REALAND_SHELL_PARENT_SOCKET")
      || !differentPrivateValue("DBUS_SESSION_BUS_ADDRESS",
                                "REALAND_SHELL_PARENT_DBUS_SESSION_BUS_ADDRESS")
      || qgetenv("QT_QPA_PLATFORM") != "wayland"
      || (!qgetenv("QT_QPA_PLATFORMTHEME").isEmpty()
          && qgetenv("QT_QPA_PLATFORMTHEME") != "kde")
      || qEnvironmentVariableIsSet("KDE_NO_GLOBAL_MENU")
      || !QFileInfo(resultPath).isAbsolute() || capturePath.isEmpty()
      || !resultPath.startsWith(QFileInfo(capturePath).absoluteFilePath() + '/')
      || !QFileInfo(registrarReadyPath).isAbsolute()
      || !registrarReadyPath.startsWith(QFileInfo(capturePath).absoluteFilePath() + '/')) {
    qCritical("Refusing non-isolated Qt menu QA environment");
    return 2;
  }
  if (!writeFile(resultPath, "0\n")) return 3;

  // Platform themes can query the cached global-menu availability during
  // QApplication/style construction. Start the actual Qt application only
  // after the shell has acquired the real private registrar, as production's
  // prepareAppMenus does before launching an application.
  QElapsedTimer registrarWaiting;
  registrarWaiting.start();
  while (!QFileInfo::exists(registrarReadyPath)) {
    if (registrarWaiting.elapsed() > 60000) {
      qCritical("Private shell registrar was not ready before Qt startup");
      return 8;
    }
    QThread::msleep(10);
  }

  QApplication application(argc, argv);
  QCoreApplication::setApplicationName("Realand Qt Menu Fixture");
  QGuiApplication::setDesktopFileName("org.test.RealandQtMenuFixture");
  if (QGuiApplication::platformName() != "wayland") return 4;

  std::array<std::unique_ptr<QMainWindow>, 2> windows;
  const std::array<QString, 2> titles = {"Realand Qt Menu Fixture",
                                        "Realand Qt Second Window"};
  for (size_t i = 0; i < windows.size(); ++i) {
    windows[i] = std::make_unique<QMainWindow>();
    windows[i]->setWindowTitle(titles[i]);
    windows[i]->resize(520, 260);
    windows[i]->setCentralWidget(new QLabel("Realand private Qt global menu fixture"));
    // Do not create QMenuBar yet: QGenericUnixTheme caches registrar availability
    // at the first menu creation. These real windows let the lazy shell host
    // observe a valid PID and acquire the registrar before Qt checks it.
    windows[i]->show();
  }

  unsigned int activations = 0;
  QString lastActivatedWindow;
  bool menusReady = false;
  const QString statePath = resultPath + ".state.json";
  QByteArray previousState;
  auto state = [&](bool registrarPresent) {
    QJsonArray windowStates;
    for (size_t i = 0; i < windows.size(); ++i) {
      // Avoid accidentally constructing a QMenuBar while waiting for the host.
      auto *bar = menusReady ? windows[i]->menuBar() : nullptr;
      windowStates.append(QJsonObject{
          {"title", titles[i]},
          {"nativeMenuBar", bar && bar->isNativeMenuBar()},
          {"localMenuBarVisible", bar && bar->isVisible()},
      });
    }
    const QByteArray contents = QJsonDocument(QJsonObject{
        {"pid", static_cast<qint64>(QCoreApplication::applicationPid())},
        {"platform", QGuiApplication::platformName()},
        {"platformTheme", qEnvironmentVariable("QT_QPA_PLATFORMTHEME")},
        {"menusReady", menusReady},
        {"dontUseNativeMenuBar", application.testAttribute(Qt::AA_DontUseNativeMenuBar)},
        {"registrarPresent", registrarPresent},
        {"activations", static_cast<qint64>(activations)},
        {"lastActivatedWindow", lastActivatedWindow},
        {"windows", windowStates},
    }).toJson(QJsonDocument::Compact);
    if (contents != previousState) {
      if (!writeFile(statePath, contents)) {
        qCritical("Cannot write private Qt menu evidence");
        application.exit(5);
      }
      previousState = contents;
    }
  };

  QElapsedTimer waiting;
  waiting.start();
  QTimer timer;
  QObject::connect(&timer, &QTimer::timeout, &application, [&] {
    auto *bus = QDBusConnection::sessionBus().interface();
    const bool registrarPresent = bus
        && bus->isServiceRegistered("com.canonical.AppMenu.Registrar").value();
    if (!menusReady && registrarPresent) {
      for (size_t i = 0; i < windows.size(); ++i) {
        auto *bar = windows[i]->menuBar();
        auto *file = bar->addMenu("&File");
        auto *verify = file->addAction(QString::fromUtf8("验证动作"));
        QObject::connect(verify, &QAction::triggered, &application, [&, i] {
          ++activations;
          lastActivatedWindow = titles[i];
          if (!writeFile(resultPath, QByteArray::number(activations) + '\n')) {
            qCritical("Cannot write private Qt QAction result");
            application.exit(6);
          }
          qInfo("Real Qt QAction activated: %u, window %zu", activations, i);
        });
        auto *edit = bar->addMenu(QString::fromUtf8("&编辑"));
        auto *disabled = edit->addAction(QString::fromUtf8("不可用"));
        disabled->setEnabled(false);
      }
      menusReady = true;
    }
    state(registrarPresent);
    if (!menusReady && waiting.elapsed() > 60000) {
      qCritical("Private shell did not acquire the menu registrar before Qt menu creation");
      application.exit(7);
    }
  });
  timer.start(50);
  state(false);
  return application.exec();
}
