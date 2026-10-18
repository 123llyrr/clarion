cmake_minimum_required(VERSION 3.13)
project(runner LANGUAGES C CXX)

find_program(WAYLAND_SCANNER wayland-scanner)
if(NOT WAYLAND_SCANNER)
  message(FATAL_ERROR "wayland-scanner is required to build the shell background-effect client")
endif()
set(BACKGROUND_EFFECT_XML "${CMAKE_CURRENT_SOURCE_DIR}/protocols/ext-background-effect-v1.xml")
set(BACKGROUND_EFFECT_HEADER "${CMAKE_CURRENT_BINARY_DIR}/ext-background-effect-v1-client-protocol.h")
set(BACKGROUND_EFFECT_CODE "${CMAKE_CURRENT_BINARY_DIR}/ext-background-effect-v1-protocol.c")
add_custom_command(OUTPUT "${BACKGROUND_EFFECT_HEADER}" "${BACKGROUND_EFFECT_CODE}"
  COMMAND "${WAYLAND_SCANNER}" client-header "${BACKGROUND_EFFECT_XML}" "${BACKGROUND_EFFECT_HEADER}"
  COMMAND "${WAYLAND_SCANNER}" private-code "${BACKGROUND_EFFECT_XML}" "${BACKGROUND_EFFECT_CODE}"
  DEPENDS "${BACKGROUND_EFFECT_XML}" VERBATIM)

set(DATA_CONTROL_XML "${CMAKE_CURRENT_SOURCE_DIR}/protocols/ext-data-control-v1.xml")
set(DATA_CONTROL_HEADER "${CMAKE_CURRENT_BINARY_DIR}/ext-data-control-v1-client-protocol.h")
set(DATA_CONTROL_CODE "${CMAKE_CURRENT_BINARY_DIR}/ext-data-control-v1-protocol.c")
add_custom_command(OUTPUT "${DATA_CONTROL_HEADER}" "${DATA_CONTROL_CODE}"
  COMMAND "${WAYLAND_SCANNER}" client-header "${DATA_CONTROL_XML}" "${DATA_CONTROL_HEADER}"
  COMMAND "${WAYLAND_SCANNER}" private-code "${DATA_CONTROL_XML}" "${DATA_CONTROL_CODE}"
  DEPENDS "${DATA_CONTROL_XML}" VERBATIM)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME}
  "main.cc"
  "shell_application.cc"
  "desktop_windows.cc"
  "window_controller.cc"
  "native_tray_menu.cc"
  "native_clipboard.cc"
  "background_effect.cc"
  "${BACKGROUND_EFFECT_HEADER}"
  "${BACKGROUND_EFFECT_CODE}"
  "${DATA_CONTROL_HEADER}"
  "${DATA_CONTROL_CODE}"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# Add preprocessor definitions for the application ID.
add_definitions(-DAPPLICATION_ID="${APPLICATION_ID}")

# Add dependency libraries. Add any application-specific dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter)
target_link_libraries(${BINARY_NAME} PRIVATE PkgConfig::GTK)
target_link_libraries(${BINARY_NAME} PRIVATE PkgConfig::LAYER_SHELL)
pkg_check_modules(DBUSMENU_GTK REQUIRED IMPORTED_TARGET dbusmenu-gtk3-0.4)
target_link_libraries(${BINARY_NAME} PRIVATE PkgConfig::DBUSMENU_GTK)
pkg_check_modules(WAYLAND_CLIENT REQUIRED IMPORTED_TARGET wayland-client>=1.20)
target_link_libraries(${BINARY_NAME} PRIVATE PkgConfig::WAYLAND_CLIENT)

target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}" "${CMAKE_CURRENT_BINARY_DIR}")
