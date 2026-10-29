#include "../src/presentation/capture/screenshotcaptureinputblockers.h"

#include <QApplication>
#include <QCloseEvent>
#include <QMenu>
#include <QMouseEvent>
#include <QWidget>
#include <QWindow>

#include <cstdlib>
#include <iostream>

namespace {
using snow_shot::presentation::closeCaptureInputBlockers;

void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(1);
    }
}

class IndirectlyClosingModal final : public QWidget {
  protected:
    void closeEvent(QCloseEvent* event) override {
        event->ignore();
        hide();
    }
};

class RefusingWindow final : public QWidget {
  protected:
    void closeEvent(QCloseEvent* event) override {
        event->ignore();
    }
};

class MouseTrackingOverlay final : public QWidget {
  public:
    MouseTrackingOverlay() : QWidget(nullptr, Qt::Tool | Qt::FramelessWindowHint) {
        setMouseTracking(true);
        setGeometry(20, 20, 100, 100);
    }

    int mouseMoves = 0;

  protected:
    void mouseMoveEvent(QMouseEvent*) override {
        ++mouseMoves;
    }
};

void sendWindowMouseMove(QWidget& widget) {
    const QPointF local(10, 10);
    QMouseEvent event(QEvent::MouseMove, local, widget.mapToGlobal(local.toPoint()), Qt::NoButton,
                      Qt::NoButton, Qt::NoModifier);
    // Exercise Qt's window-to-widget routing, including active popup redirection.
    QApplication::sendEvent(widget.windowHandle(), &event);
}

void popupReleasesOverlayMouseInput() {
    QMenu menu;
    menu.addAction(QStringLiteral("Capture"));
    menu.popup(QPoint(200, 200));
    MouseTrackingOverlay overlay;
    overlay.show();
    QApplication::processEvents();
    require(QApplication::activePopupWidget() == &menu &&
                QApplication::activeModalWidget() == nullptr,
            "a tray-style menu owns popup input without being a modal dialog");

    overlay.mouseMoves = 0;
    sendWindowMouseMove(overlay);
    require(overlay.mouseMoves == 0,
            "an open popup redirects mouse movement away from the overlay");
    require(closeCaptureInputBlockers(), "capture preparation should release popup input");
    sendWindowMouseMove(overlay);
    require(overlay.mouseMoves == 1,
            "capture preparation must restore mouse movement to the screenshot overlay");
    require(!menu.isVisible() && QApplication::activePopupWidget() == nullptr,
            "capture preparation must dismiss the popup before showing overlays");
}

void nestedPopupsCloseBeforeTheirModalOwner() {
    QWidget modeless;
    modeless.show();
    QWidget modal;
    modal.setWindowModality(Qt::ApplicationModal);
    modal.show();
    QMenu menu(&modal);
    auto* child = menu.addMenu(QStringLiteral("Child"));
    auto* nested = child->addMenu(QStringLiteral("Nested"));
    nested->addAction(QStringLiteral("Item"));
    menu.popup(QPoint(200, 200));
    child->popup(QPoint(300, 200));
    nested->popup(QPoint(400, 200));
    require(QApplication::activeModalWidget() == &modal &&
                QApplication::activePopupWidget() == nested,
            "nested menus and their modal owner should both block capture input");

    require(closeCaptureInputBlockers(), "capture should dismiss the entire popup and modal stack");
    require(!menu.isVisible() && !child->isVisible() && !nested->isVisible() &&
                !modal.isVisible() && QApplication::activePopupWidget() == nullptr &&
                QApplication::activeModalWidget() == nullptr,
            "no nested popup or modal may retain capture input");
    require(modeless.isVisible(), "capture preparation must preserve unrelated modeless windows");
}

void modalCloseHandlerCanOpenAPopup() {
    class Modal final : public QWidget {
      public:
        QMenu popup;

      protected:
        void closeEvent(QCloseEvent* event) override {
            require(QApplication::activePopupWidget() == nullptr,
                    "capture must close popups before their modal owner");
            popup.addAction(QStringLiteral("Opened while closing"));
            popup.popup(QPoint(200, 200));
            event->accept();
        }
    } modal;
    modal.setWindowModality(Qt::ApplicationModal);
    modal.show();
    QMenu original(&modal);
    original.addAction(QStringLiteral("Original popup"));
    original.popup(QPoint(300, 300));

    require(closeCaptureInputBlockers(), "capture should close blockers created by close handlers");
    require(!original.isVisible() && !modal.popup.isVisible() && !modal.isVisible() &&
                QApplication::activePopupWidget() == nullptr &&
                QApplication::activeModalWidget() == nullptr,
            "capture must recheck popup ownership after closing a modal");
}

void refusingPopupPreventsCapture() {
    RefusingWindow popup;
    popup.setWindowFlags(Qt::Popup);
    popup.show();
    require(QApplication::activePopupWidget() == &popup,
            "the refusing window must own popup input");
    require(!closeCaptureInputBlockers(), "capture must not start when a popup refuses to close");
    require(popup.isVisible() && QApplication::activePopupWidget() == &popup,
            "a refused close must leave the popup usable");
    popup.hide();
}
} // namespace

int main(int argc, char* argv[]) {
    QApplication application(argc, argv);
    popupReleasesOverlayMouseInput();
    nestedPopupsCloseBeforeTheirModalOwner();
    modalCloseHandlerCanOpenAPopup();
    refusingPopupPreventsCapture();

    QWidget first;
    first.setWindowModality(Qt::ApplicationModal);
    first.show();
    IndirectlyClosingModal second;
    second.setWindowModality(Qt::ApplicationModal);
    second.show();
    QApplication::processEvents();
    require(QApplication::activeModalWidget() == &second,
            "the last dialog should block the screenshot overlay");

    require(closeCaptureInputBlockers(), "capture should close every active modal window");
    require(!first.isVisible() && !second.isVisible() &&
                QApplication::activeModalWidget() == nullptr,
            "closed dialogs must no longer block screenshot input");

    RefusingWindow refusing;
    refusing.setWindowModality(Qt::ApplicationModal);
    refusing.show();
    QApplication::processEvents();
    require(!closeCaptureInputBlockers(), "capture must not start when a dialog refuses to close");
    require(refusing.isVisible() && QApplication::activeModalWidget() == &refusing,
            "a refused close must leave the dialog usable");
    refusing.hide();
    return 0;
}
