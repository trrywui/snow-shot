include_guard(GLOBAL)

function(_snow_append_license_notice source label)
    if(NOT EXISTS "${source}" OR IS_DIRECTORY "${source}")
        message(FATAL_ERROR "Missing license notice: ${source}")
    endif()
    file(READ "${source}" _contents)
    string(APPEND _snow_license_text
        "\n======================================================================\n"
        "FILE: ${label}\n"
        "======================================================================\n\n"
        "${_contents}\n")
    set(_snow_license_text "${_snow_license_text}" PARENT_SCOPE)
endfunction()

# Keep the collector's complete audit tree in the build directory. Only this
# readable document is installed; section labels preserve the original mapping.
function(snow_write_license_bundle)
    cmake_parse_arguments(PARSE_ARGV 0 NOTICES ""
        "SOURCE_DIRECTORY;THIRD_PARTY_DIRECTORY;OUTPUT;PRODUCT" "")
    string(CONCAT _snow_license_text
        "${NOTICES_PRODUCT} - Licenses and Notices\n\n"
        "This document preserves the collected license, copyright, attribution,\n"
        "and provenance texts. Paths identify sections of this document rather\n"
        "than separate installed files. The original texts follow each FILE label.\n")
    foreach(_name IN ITEMS LICENSE COPYRIGHT THIRD_PARTY_NOTICES.md)
        _snow_append_license_notice("${NOTICES_SOURCE_DIRECTORY}/${_name}" "${_name}")
    endforeach()
    foreach(_project IN ITEMS snow_image ant_design_qt snow-crates snow_draw_engine_qt snow_rust_ffi)
        string(REPLACE "_" "-" _label "${_project}")
        foreach(_name IN ITEMS LICENSE COPYRIGHT)
            _snow_append_license_notice("${NOTICES_SOURCE_DIRECTORY}/../${_project}/${_name}"
                "components/${_label}/${_name}")
        endforeach()
    endforeach()
    foreach(_required IN ITEMS INDEX.md manifest.json
            project-notices/ant-design-icons/THIRD_PARTY_NOTICES.md)
        if(NOT EXISTS "${NOTICES_THIRD_PARTY_DIRECTORY}/${_required}")
            message(FATAL_ERROR "Missing collected license bundle entry: ${_required}")
        endif()
    endforeach()
    file(READ "${NOTICES_THIRD_PARTY_DIRECTORY}/manifest.json" _manifest)
    string(JSON _notice_count LENGTH "${_manifest}" Notices)
    if(_notice_count EQUAL 0)
        message(FATAL_ERROR "The collected license manifest contains no notices")
    endif()
    math(EXPR _last_notice "${_notice_count} - 1")
    foreach(_index RANGE 0 ${_last_notice})
        string(JSON _notice GET "${_manifest}" Notices ${_index} Notice)
        if(NOT EXISTS "${NOTICES_THIRD_PARTY_DIRECTORY}/${_notice}")
            message(FATAL_ERROR "Missing collected license notice: ${_notice}")
        endif()
    endforeach()
    file(GLOB_RECURSE _notices LIST_DIRECTORIES false
        RELATIVE "${NOTICES_THIRD_PARTY_DIRECTORY}" "${NOTICES_THIRD_PARTY_DIRECTORY}/*")
    list(SORT _notices)
    foreach(_notice IN LISTS _notices)
        _snow_append_license_notice("${NOTICES_THIRD_PARTY_DIRECTORY}/${_notice}"
            "third-party/${_notice}")
    endforeach()
    # Substitute the whole document once so literal ${...} and @...@ in upstream
    # notices are preserved. Force LF rather than Windows' native text newlines.
    file(CONFIGURE OUTPUT "${NOTICES_OUTPUT}" CONTENT "@_snow_license_text@"
        @ONLY NEWLINE_STYLE LF)
endfunction()

function(snow_install_license_bundle)
    cmake_parse_arguments(PARSE_ARGV 0 NOTICES ""
        "SOURCE_DIRECTORY;THIRD_PARTY_DIRECTORY;DESTINATION;COMPONENT;PRODUCT" "")
    set(_script "${CMAKE_CURRENT_BINARY_DIR}/InstallLicenseBundle-${NOTICES_COMPONENT}.cmake")
    configure_file("${CMAKE_CURRENT_FUNCTION_LIST_DIR}/InstallSnowShotLicenseBundle.cmake.in"
        "${_script}" @ONLY)
    install(SCRIPT "${_script}" COMPONENT "${NOTICES_COMPONENT}")
endfunction()
