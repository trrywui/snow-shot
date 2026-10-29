include_guard(DIRECTORY)

function(snow_shot_apply_clipboard_test_resources)
    if(NOT WIN32)
        return()
    endif()

    # The Windows image publisher uses the system clipboard even with the offscreen QPA.
    # Keep every alias of the shared hosts together. The broad pinned family intentionally
    # includes its related repository/geometry fixtures so new pinned aliases stay protected.
    set(_clipboard_test_families
        "^snow-shot-.*pinned-"
        "^snow-shot-image-edit(or|ing)-"
        "^snow-shot-clipboard-(placement|roundtrip|appearance-presentation)-"
        "^snow-shot-export-(coordinator|scheduling)-"
        "^snow-shot-screenshot-history-(page|context-menu)-"
        "^snow-shot-(file-pin-presentation|duplicate-pin|pin-execution-autosave)-"
        "^snow-shot-(collage-history-selection|history-selection-presentation)-"
        "^snow-shot-(decoded-content-presentation|selection-clipboard-lifecycle|layout-compaction-pin)-")
    get_property(_tests DIRECTORY PROPERTY TESTS)
    foreach(_test IN LISTS _tests)
        foreach(_family IN LISTS _clipboard_test_families)
            if(_test MATCHES "${_family}")
                set_property(TEST "${_test}" APPEND PROPERTY RESOURCE_LOCK
                    snow-shot-system-clipboard)
                break()
            endif()
        endforeach()
    endforeach()
endfunction()

# Also cover aliases registered later in the directory or by an included test module.
cmake_language(DEFER CALL snow_shot_apply_clipboard_test_resources)
