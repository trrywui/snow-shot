cmake_minimum_required(VERSION 4.2)
project(snow_macos14_personal_tests LANGUAGES CXX)
find_package(Qt6 6.12.0 EXACT REQUIRED COMPONENTS Core Network)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_AUTOMOC ON)
enable_testing()
get_filename_component(SNOW_REPO "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
file(READ "${SNOW_REPO}/snow_shot/packaging/macos/maintained-release.json" _release)
string(JSON _version GET "${_release}" version)
string(JSON _upstream GET "${_release}" upstreamVersion)
add_executable(snow-shot-macos14-personal-tests
    "${SNOW_REPO}/snow_shot/tests/macos14_personal_tests.cpp"
    "${SNOW_REPO}/snow_shot/src/update/macosupdateservice.cpp"
    "${SNOW_REPO}/snow_shot/include/snow_shot/update/updateservice.h")
target_include_directories(snow-shot-macos14-personal-tests PRIVATE "${SNOW_REPO}/snow_shot/include")
target_link_libraries(snow-shot-macos14-personal-tests PRIVATE Qt6::Core Qt6::Network)
target_compile_definitions(snow-shot-macos14-personal-tests PRIVATE
    SNOW_SHOT_MACOS14_PERSONAL=1 SNOW_SHOT_VERSION="${_version}+up.${_upstream}")
qt_import_plugins(snow-shot-macos14-personal-tests NO_DEFAULT)
add_test(NAME snow-shot-macos14-personal-tests COMMAND snow-shot-macos14-personal-tests)

# Exercise the official update behavior without the personal compile definition.
add_executable(snow-shot-macos-upstream-update-tests
    "${SNOW_REPO}/snow_shot/tests/macos_update_tests.cpp"
    "${SNOW_REPO}/snow_shot/src/update/macosupdateservice.cpp"
    "${SNOW_REPO}/snow_shot/include/snow_shot/update/updateservice.h")
target_include_directories(snow-shot-macos-upstream-update-tests PRIVATE "${SNOW_REPO}/snow_shot/include")
target_link_libraries(snow-shot-macos-upstream-update-tests PRIVATE Qt6::Core Qt6::Network)
target_compile_definitions(snow-shot-macos-upstream-update-tests PRIVATE SNOW_SHOT_VERSION="1.2.8")
qt_import_plugins(snow-shot-macos-upstream-update-tests NO_DEFAULT)
add_test(NAME snow-shot-macos-upstream-update-tests COMMAND snow-shot-macos-upstream-update-tests)
