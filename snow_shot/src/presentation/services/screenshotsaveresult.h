#pragma once

#include "snow_shot/presentation/screenshotclipboardservice.h"
#include "snow_shot/presentation/screenshotexportcoordinator.h"

#include <QCoreApplication>
#include <QObject>
#include <QPointer>

class QLabel;
class QWidget;
namespace adqt::widgets {
class AdButton;
class AdResult;
class AdScrollArea;
} // namespace adqt::widgets
namespace screenshot_save_export {
struct PreparedPixels;
}

// Owns the successful output snapshot and the lifetime of its post-save actions.
// The dialog owns export editing and completion; this component never finishes an export.
class ScreenshotSaveResult final : public QObject {
    Q_DECLARE_TR_FUNCTIONS(ScreenshotSaveAsFileDialog)

  public:
    struct Output {
        QString path;
        std::shared_ptr<const screenshot_save_export::PreparedPixels> pixels;
        ScreenshotImageEncodingOptions encoding;
    };
    ScreenshotSaveResult(QWidget* parent, std::function<void()> continueEditing,
                         std::function<void()> dismiss);
    ~ScreenshotSaveResult() override;
    QWidget* view() const;
    QWidget* actions() const;
    void present(Output output);
    void cancel();
    void activateDefaultAction();
    void focusDefaultAction();

  protected:
    bool eventFilter(QObject* object, QEvent* event) override;

  private:
    enum class State { Inactive, Ready, Busy, Closed };
    enum class Error { None, MissingFile, ClipboardPreparation, Queue, Clipboard, Folder };
    void retranslate();
    void updateError();
    void updateActions();
    bool beginAction(adqt::widgets::AdButton* button);
    void fail(Error error);
    void clipboardFinished(ScreenshotClipboardCommitResult result);
    void copyFile();
    void copyPath();

    const std::function<void()> m_continueEditing;
    const std::function<void()> m_dismiss;
    Output m_output;
    State m_state = State::Inactive;
    Error m_error = Error::None;
    ScreenshotExportJobHandle m_job;
    ScreenshotClipboardCommitScope m_clipboard;
    ScreenshotClipboardCommitResult m_clipboardFailure;
    QPointer<adqt::widgets::AdScrollArea> m_view;
    QPointer<QWidget> m_actions;
    adqt::widgets::AdResult* m_result = nullptr;
    QLabel* m_errorLabel = nullptr;
    adqt::widgets::AdButton* m_continue = nullptr;
    adqt::widgets::AdButton* m_copyFile = nullptr;
    adqt::widgets::AdButton* m_copyPath = nullptr;
    adqt::widgets::AdButton* m_openFolder = nullptr;
    QPointer<adqt::widgets::AdButton> m_busyButton;
};
