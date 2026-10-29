#ifndef SNOW_SHOT_PLATFORM_WINDOWS_AUTOSTARTREGISTRATION_H
#define SNOW_SHOT_PLATFORM_WINDOWS_AUTOSTARTREGISTRATION_H

#include <QByteArray>
#include <QString>
#include <QtTypes>

namespace snow_shot::platform::windows {

enum class AutoStartState {
    Unavailable,
    Disabled,
    Enabled,
    DisabledByUser,
    DisabledByPolicy,
    EnabledByPolicy
};

struct AutoStartRegistrationSnapshot {
    bool valid = false;
    bool exists = false;
    quint32 nativeType = 0;
    QByteArray nativeData;
    QString error;
    AutoStartState state = AutoStartState::Unavailable;
};

class AutoStartRegistration final {
  public:
    [[nodiscard]] static constexpr bool canChangeState(AutoStartState state) {
        return state == AutoStartState::Disabled || state == AutoStartState::Enabled;
    }
    [[nodiscard]] static bool isSupported();
    [[nodiscard]] static QString hint();
    [[nodiscard]] static QString expectedCommand();
    [[nodiscard]] static AutoStartRegistrationSnapshot snapshot();
    [[nodiscard]] static bool matchesExpectedCommand();
    [[nodiscard]] static bool setEnabled(bool enabled, QString* error = nullptr);
    [[nodiscard]] static bool restore(const AutoStartRegistrationSnapshot& snapshot,
                                      QString* error = nullptr);
};

} // namespace snow_shot::platform::windows

#endif // SNOW_SHOT_PLATFORM_WINDOWS_AUTOSTARTREGISTRATION_H
