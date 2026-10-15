# Personal compatibility builds never share macOS 15 dependency artifacts.
include("${CMAKE_CURRENT_LIST_DIR}/x64-osx-snow-shot-static.cmake")
set(VCPKG_OSX_DEPLOYMENT_TARGET "14.4")
