#include "snow_shot/app/distribution.h"

#ifdef Q_OS_WIN
#include <qt_windows.h>
#include <appmodel.h>
#include <string>
#endif

namespace snow_shot::app::distribution {

QString runtimePackageFamilyName() {
#ifdef Q_OS_WIN
    UINT32 length = 0;
    if (GetCurrentPackageFamilyName(&length, nullptr) != ERROR_INSUFFICIENT_BUFFER || length == 0)
        return {};
    std::wstring family(length, L'\0');
    if (GetCurrentPackageFamilyName(&length, family.data()) == ERROR_SUCCESS)
        return QString::fromWCharArray(family.c_str());
#endif
    return {};
}

QString localApplicationDataDirectory() {
#ifdef Q_OS_WIN
    const DWORD length = GetEnvironmentVariableW(L"LOCALAPPDATA", nullptr, 0);
    if (length == 0)
        return {};
    std::wstring path(length, L'\0');
    if (GetEnvironmentVariableW(L"LOCALAPPDATA", path.data(), length) == 0)
        return {};
    const auto directory = QDir::fromNativeSeparators(QString::fromWCharArray(path.c_str()));
    return QDir::isAbsolutePath(directory) ? directory : QString();
#else
    return {};
#endif
}

} // namespace snow_shot::app::distribution
