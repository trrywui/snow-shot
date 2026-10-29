cmake_minimum_required(VERSION 3.25)
project(SnowQtGraphicsViewContract LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
include("${CMAKE_CURRENT_SOURCE_DIR}/../../cmake/SnowQt.cmake")
include("${CMAKE_CURRENT_SOURCE_DIR}/../../cmake/SnowQtOffscreenTest.cmake")
include("${CMAKE_CURRENT_SOURCE_DIR}/../../cmake/ProjectWarnings.cmake")
snow_find_qt(COMPONENTS Widgets)
add_executable(snow-shot-qt-graphicsview-rendering-tests
    ../../snow_shot/tests/qt_graphicsview_rendering_tests.cpp)
target_link_libraries(snow-shot-qt-graphicsview-rendering-tests PRIVATE Qt6::Widgets)
snow_shot_import_offscreen_platform(snow-shot-qt-graphicsview-rendering-tests)
snow_apply_strict_warnings(snow-shot-qt-graphicsview-rendering-tests)
snow_shot_offscreen_qpa_environment(qpa_environment)
enable_testing()
add_test(NAME snow-shot-qt-graphicsview-rendering-tests
    COMMAND snow-shot-qt-graphicsview-rendering-tests -platform offscreen)
set_tests_properties(snow-shot-qt-graphicsview-rendering-tests PROPERTIES
    LABELS unit TIMEOUT 30 ENVIRONMENT "${qpa_environment}")
