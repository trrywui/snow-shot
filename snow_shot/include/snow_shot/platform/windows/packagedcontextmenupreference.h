#ifndef SNOW_SHOT_PLATFORM_WINDOWS_PACKAGEDCONTEXTMENUPREFERENCE_H
#define SNOW_SHOT_PLATFORM_WINDOWS_PACKAGEDCONTEXTMENUPREFERENCE_H

#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <appmodel.h>

#include <string>

namespace snow_shot::platform::windows::packaged_context_menu {

struct Preference {
    bool enabled = true;
    wchar_t accessKey = L'\0';

    friend bool operator==(const Preference&, const Preference&) = default;
};

inline bool isAccessKey(wchar_t key) {
    return (key >= L'A' && key <= L'Z') || (key >= L'0' && key <= L'9');
}

// A small, Qt-independent preference shared with the packaged COM surrogate.
// LocalState survives package updates and is independent of custom app storage.
inline std::wstring preferencePath() {
    UINT32 familyLength = 0;
    if (GetCurrentPackageFamilyName(&familyLength, nullptr) != ERROR_INSUFFICIENT_BUFFER)
        return {};
    std::wstring family(familyLength, L'\0');
    if (GetCurrentPackageFamilyName(&familyLength, family.data()) != ERROR_SUCCESS)
        return {};
    family.resize(familyLength - 1);
    const DWORD length = GetEnvironmentVariableW(L"LOCALAPPDATA", nullptr, 0);
    if (length == 0)
        return {};
    std::wstring local(length, L'\0');
    const DWORD written = GetEnvironmentVariableW(L"LOCALAPPDATA", local.data(), length);
    if (written == 0 || written >= length)
        return {};
    local.resize(written);
    return local + L"\\Packages\\" + family + L"\\LocalState\\context-menu-enabled";
}

inline Preference readPreference(const std::wstring& path) {
    if (path.empty())
        return {false};
    const HANDLE file = CreateFileW(path.c_str(), GENERIC_READ,
                                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        const DWORD error = GetLastError();
        // Preserve the existing enabled-by-default behavior before the first app launch.
        return {error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND};
    }
    // The original one-byte mode remains valid. An optional second byte stores
    // the canonical ASCII access key; the third byte detects malformed payloads.
    char value[3]{};
    DWORD read = 0;
    const bool success = ReadFile(file, value, sizeof(value), &read, nullptr) != FALSE;
    CloseHandle(file);
    if (!success || (read != 1 && read != 2) || (value[0] != '0' && value[0] != '1') ||
        (read == 2 && !isAccessKey(value[1])))
        return {false};
    return {value[0] == '1', read == 2 ? static_cast<wchar_t>(value[1]) : L'\0'};
}

inline bool isEnabled(const std::wstring& path) {
    return readPreference(path).enabled;
}

} // namespace snow_shot::platform::windows::packaged_context_menu

#endif
