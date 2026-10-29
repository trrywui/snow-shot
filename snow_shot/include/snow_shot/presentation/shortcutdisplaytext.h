#pragma once

#include "snow_shot/shortcuts/shortcutdisplayservice.h"

namespace snow_shot::presentation {

inline QString formatShortcutDisplayText(const shortcuts::ShortcutBinding& shortcut) {
    return shortcuts::formatShortcutDisplayText(shortcut, shortcuts::ShortcutPolicy::Application);
}

inline QString formatShortcutListDisplayText(const shortcuts::ShortcutBindingList& shortcuts) {
    return shortcuts::formatShortcutListDisplayText(shortcuts,
                                                    shortcuts::ShortcutPolicy::Application);
}

} // namespace snow_shot::presentation
