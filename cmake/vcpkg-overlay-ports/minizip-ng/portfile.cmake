if(VCPKG_TARGET_IS_WINDOWS)
    vcpkg_check_linkage(ONLY_STATIC_LIBRARY)
    # Snow Shot links the static Qt kit and therefore uses the static CRT even
    # in its debug preset. minizip is linked into that executable directly.
    set(VCPKG_CRT_LINKAGE static)
endif()

vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO zlib-ng/minizip-ng
    REF "${VERSION}"
    SHA512 9ea5dde14acd2f7d1efd0e38b11017b679d3aaabac61552f9c5f4c7f45f2563543e0fbb2d74429c6b1b9c37d8728ebc4f1cf0efad5f71807c11bb8a2a681a556
    HEAD_REF master
)

vcpkg_check_features(
    OUT_FEATURE_OPTIONS FEATURE_OPTIONS
    FEATURES zlib MZ_ZLIB
)

vcpkg_cmake_configure(
    SOURCE_PATH "${SOURCE_PATH}"
    OPTIONS
        ${FEATURE_OPTIONS}
        -DMZ_FETCH_LIBS=OFF
        -DMZ_LIB_SUFFIX=-ng
        -DMZ_ICONV=OFF
        -DMZ_COMPAT=OFF
        # Only the declared zlib feature is supplied by this port. Prevent
        # auto-detection from linking unrelated host packages into static apps.
        # Apple encryption continues to use the native CommonCrypto backend.
        -DMZ_BZIP2=OFF
        -DMZ_LZMA=OFF
        -DMZ_ZSTD=OFF
        -DMZ_OPENSSL=OFF
    OPTIONS_RELEASE
        -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
    OPTIONS_DEBUG
        -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDebug
)
vcpkg_cmake_install()
vcpkg_fixup_pkgconfig()
vcpkg_cmake_config_fixup(CONFIG_PATH lib/cmake/minizip-ng)
if(VCPKG_TARGET_IS_OSX)
    # Binary caches must not export the builder's SDK into consumers. An SDK
    # framework path adds a non-system -F search path, exposing Apple's headers
    # to project warnings when the consumer selects a different Xcode/CLT SDK.
    set(_minizip_targets "${CURRENT_PACKAGES_DIR}/share/minizip-ng/minizip-ng.cmake")
    file(READ "${_minizip_targets}" _minizip_export)
    string(REGEX REPLACE
        [[/[^;"]+[.]sdk/System/Library/Frameworks/([A-Za-z0-9_]+)[.]framework]]
        [[-framework \1]] _minizip_export "${_minizip_export}")
    # The config already calls find_dependency(BZip2); retain its consumer-side
    # resolution instead of the absolute libbz2.tbd captured during packaging.
    string(REGEX REPLACE [[/[^;"]+[.]sdk/usr/lib/libbz2[.]tbd]]
        [[BZip2::BZip2]] _minizip_export "${_minizip_export}")
    file(WRITE "${_minizip_targets}" "${_minizip_export}")
endif()
vcpkg_copy_pdbs()
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/share" "${CURRENT_PACKAGES_DIR}/debug/include")
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
