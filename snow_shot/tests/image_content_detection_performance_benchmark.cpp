#include "snowimageqtcodec.h"

#include <QBuffer>
#include <QCoreApplication>
#include <QElapsedTimer>

#include <algorithm>
#include <array>
#include <cstdlib>
#include <iostream>

namespace {
void require(bool condition, const char* message) {
    if (!condition) {
        std::cerr << message << '\n';
        std::exit(EXIT_FAILURE);
    }
}
} // namespace

int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    constexpr int samples = 31;
    std::cout << "format,width,height,encoded_bytes,strict_median_ms,auto_median_ms\n";
    for (const auto format : {snow::image::Format::jpeg, snow::image::Format::png}) {
        for (const auto size : {QSize(64, 64), QSize(1920, 1080), QSize(3840, 2160)}) {
            QImage source(size, QImage::Format_RGBA8888);
            require(!source.isNull(), "allocate fixture");
            for (int y = 0; y < size.height(); ++y) {
                auto* row = source.scanLine(y);
                for (int x = 0; x < size.width(); ++x) {
                    row[x * 4] = static_cast<uchar>((x * 17 + y * 3) & 255);
                    row[x * 4 + 1] = static_cast<uchar>((x + y * 11) & 255);
                    row[x * 4 + 2] = static_cast<uchar>((x * 7 + y) & 255);
                    row[x * 4 + 3] = 255;
                }
            }
            QByteArray encoded;
            QBuffer buffer(&encoded);
            require(buffer.open(QIODevice::WriteOnly) &&
                        snow_shot::image_codec::encodeToDevice(source, &buffer, format, {}),
                    "encode fixture");
            const auto reference = snow_shot::image_codec::decode(encoded, format, nullptr);
            require(!reference.isNull(), "decode reference");
            std::array<std::array<double, samples>, 2> times{};
            // Alternate order to reduce cache/thermal bias. Exclude warmups and validation.
            for (int sample = -3; sample < samples; ++sample) {
                for (int turn = 0; turn < 2; ++turn) {
                    const int mode = (sample + 4 + turn) % 2;
                    QElapsedTimer timer;
                    timer.start();
                    const auto decoded = snow_shot::image_codec::decode(
                        encoded, mode == 0 ? format : snow::image::Format::unknown, nullptr);
                    const auto elapsed = timer.nsecsElapsed();
                    require(decoded == reference, "automatic and strict decodes must match");
                    if (sample >= 0)
                        times[mode][sample] = static_cast<double>(elapsed) / 1e6;
                }
            }
            for (auto& series : times)
                std::sort(series.begin(), series.end());
            std::cout << (format == snow::image::Format::jpeg ? "jpeg" : "png") << ','
                      << size.width() << ',' << size.height() << ',' << encoded.size() << ','
                      << times[0][samples / 2] << ',' << times[1][samples / 2] << '\n';
        }
    }
    return EXIT_SUCCESS;
}
