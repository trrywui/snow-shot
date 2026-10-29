# A source's external implementation requirements belong to that source, including
# when a focused fixture or the Mini product compiles a copy of its source list.
include_guard(DIRECTORY)

function(snow_shot_register_source_capability capability)
    cmake_parse_arguments(PARSE_ARGV 1 _provider "" "FULL_TARGET;MINI_TARGET" "")
    if(_provider_UNPARSED_ARGUMENTS OR _provider_KEYWORDS_MISSING_VALUES OR
            NOT capability MATCHES "^[a-z][a-z0-9_]*$" OR NOT _provider_FULL_TARGET)
        message(FATAL_ERROR "Source capability registration requires a name and FULL_TARGET")
    endif()
    get_property(_registered DIRECTORY PROPERTY "SNOW_SHOT_SOURCE_CAPABILITY_${capability}" SET)
    if(_registered)
        message(FATAL_ERROR "Source capability '${capability}' is already registered")
    endif()
    # Providers may be declared later; their existence is checked after all consumers
    # and edition variants have been created. Mini never falls back to a Full provider.
    set_property(DIRECTORY PROPERTY "SNOW_SHOT_SOURCE_CAPABILITY_${capability}" TRUE)
    set_property(DIRECTORY PROPERTY "SNOW_SHOT_SOURCE_CAPABILITY_${capability}_full"
        "${_provider_FULL_TARGET}")
    set_property(DIRECTORY PROPERTY "SNOW_SHOT_SOURCE_CAPABILITY_${capability}_mini"
        "${_provider_MINI_TARGET}")
endfunction()

function(snow_shot_source_requires capability)
    cmake_parse_arguments(PARSE_ARGV 1 _requirement "" "" "SOURCES")
    if(_requirement_UNPARSED_ARGUMENTS OR _requirement_KEYWORDS_MISSING_VALUES OR
            NOT capability MATCHES "^[a-z][a-z0-9_]*$" OR NOT _requirement_SOURCES)
        message(FATAL_ERROR "Source requirements need a capability name and SOURCES")
    endif()
    foreach(_source IN LISTS _requirement_SOURCES)
        set_property(SOURCE "${_source}" APPEND PROPERTY
            SNOW_SHOT_REQUIRED_SOURCE_CAPABILITIES "${capability}")
    endforeach()
endfunction()

function(snow_shot_set_source_dependency_edition target edition)
    if(NOT TARGET "${target}" OR NOT edition MATCHES "^(full|mini)$")
        message(FATAL_ERROR
            "Source dependency edition requires an existing target and full or mini")
    endif()
    set_property(TARGET "${target}" PROPERTY SNOW_SHOT_SOURCE_DEPENDENCY_EDITION "${edition}")
endfunction()

# Call after declarations, before the final Mini graph audit. Source-file properties
# use CMake's exact path identity rather than inferring ownership from basenames.
# Ordinary consumers use Full; the Mini cloning helper explicitly marks each clone.
function(snow_shot_resolve_source_dependencies)
    get_property(_targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
    foreach(_target IN LISTS _targets)
        get_target_property(_edition "${_target}" SNOW_SHOT_SOURCE_DEPENDENCY_EDITION)
        if(NOT _edition)
            set(_edition full)
        elseif(NOT _edition MATCHES "^(full|mini)$")
            message(FATAL_ERROR
                "Target '${_target}' has invalid source dependency edition '${_edition}'")
        endif()
        set(_capabilities)
        foreach(_source_property IN ITEMS SOURCES INTERFACE_SOURCES)
            set(_capabilities_${_source_property})
            get_target_property(_sources "${_target}" "${_source_property}")
            foreach(_source IN LISTS _sources)
                get_source_file_property(_required "${_source}" TARGET_DIRECTORY "${_target}"
                    SNOW_SHOT_REQUIRED_SOURCE_CAPABILITIES)
                if(_required)
                    list(APPEND _capabilities_${_source_property} ${_required})
                    list(APPEND _capabilities ${_required})
                endif()
            endforeach()
        endforeach()
        list(REMOVE_DUPLICATES _capabilities)
        foreach(_capability IN LISTS _capabilities)
            get_property(_registered DIRECTORY PROPERTY
                "SNOW_SHOT_SOURCE_CAPABILITY_${_capability}" SET)
            if(NOT _registered)
                message(FATAL_ERROR
                    "Target '${_target}' requires unregistered source capability '${_capability}'")
            endif()
            get_property(_provider DIRECTORY PROPERTY
                "SNOW_SHOT_SOURCE_CAPABILITY_${_capability}_${_edition}")
            if(NOT _provider)
                message(FATAL_ERROR
                    "Source capability '${_capability}' has no ${_edition} provider "
                    "for '${_target}'")
            elseif(NOT TARGET "${_provider}")
                message(FATAL_ERROR
                    "Source capability '${_capability}' requires missing provider target "
                    "'${_provider}'")
            endif()
            get_target_property(_type "${_target}" TYPE)
            if(_type STREQUAL "INTERFACE_LIBRARY" OR
                    _capability IN_LIST _capabilities_INTERFACE_SOURCES)
                get_target_property(_links "${_target}" INTERFACE_LINK_LIBRARIES)
                if(NOT _provider IN_LIST _links)
                    target_link_libraries("${_target}" INTERFACE "${_provider}")
                endif()
            endif()
            if(NOT _type STREQUAL "INTERFACE_LIBRARY" AND
                    _capability IN_LIST _capabilities_SOURCES)
                get_target_property(_links "${_target}" LINK_LIBRARIES)
                if(NOT _provider IN_LIST _links)
                    target_link_libraries("${_target}" PRIVATE "${_provider}")
                endif()
            endif()
        endforeach()
    endforeach()
endfunction()
