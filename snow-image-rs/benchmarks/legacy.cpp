// GPL-3.0-or-later. Standalone benchmark; does not alter the legacy application.
#include <snow/image/processing.h>
#include <snow/image/service.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using namespace snow::image;
constexpr PixelFormat kPremultipliedRgba8{SampleType::unsigned_integer, ChannelLayout::rgba,
                                        AlphaMode::premultiplied, 8, true};

template <class T> T take(Result<T> value) {
    if (!value)
        throw std::runtime_error(value.error().message);
    return std::move(value).value();
}

void check(Result<void> value) {
    if (!value)
        throw std::runtime_error(value.error().message);
}

std::array<std::byte, 4> pattern(std::uint32_t x, std::uint32_t y, bool alpha) {
    const bool stroke = y % 24U < 13U && x % 11U < 2U;
    const std::array<std::uint8_t, 3> color =
        stroke ? std::array<std::uint8_t, 3>{16, 83, 219}
        : y % 180U < 36U
            ? std::array<std::uint8_t, 3>{31, 41, 55}
            : std::array<std::uint8_t, 3>{static_cast<std::uint8_t>(232U + x % 23U),
                                          static_cast<std::uint8_t>(230U + y % 25U), 240};
    return {std::byte{color[0]}, std::byte{color[1]}, std::byte{color[2]},
            std::byte{static_cast<std::uint8_t>(alpha ? (x * 3U + y) % 256U : 255U)}};
}

Document fixture(std::uint32_t width, std::uint32_t height, bool alpha) {
    auto image = take(MutableImage::allocate(width, height, kRgba8));
    auto pixels = image.pixels();
    for (std::uint32_t y = 0; y < height; ++y) {
        for (std::uint32_t x = 0; x < width; ++x) {
            const auto offset = static_cast<std::size_t>(y) * image.row_stride() + x * 4U;
            const auto pixel = pattern(x, y, alpha);
            std::copy(pixel.begin(), pixel.end(),
                      pixels.begin() + static_cast<std::ptrdiff_t>(offset));
        }
    }
    Document document;
    document.format = Format::png;
    document.canvas_width = width;
    document.canvas_height = height;
    document.color.primaries = ColorPrimaries::srgb;
    document.color.transfer = TransferFunction::srgb;
    Frame frame;
    frame.image = std::move(image).freeze();
    frame.color = document.color;
    document.frames.push_back(std::move(frame));
    return document;
}

class Source final : public RasterSource {
  public:
    explicit Source(const Document& document)
        : descriptor_(take(describe_document(document))), image_(document.frames.at(0).image) {}
    const DocumentDescriptor& descriptor() const noexcept override {
        return descriptor_;
    }
    RasterAccess access() const noexcept override {
        return RasterAccess::random_rows | RasterAccess::mapped_planes |
               RasterAccess::concurrent_reads;
    }
    Result<void> read_rows(std::uint32_t frame, std::uint32_t plane, std::uint32_t first,
                           std::uint32_t rows, std::size_t stride, std::span<std::byte> destination,
                           std::stop_token stop) const override {
        const std::size_t row_bytes = static_cast<std::size_t>(image_.width()) * 4U;
        if (frame != 0 || plane != 0 || first > image_.height() || rows > image_.height() - first ||
            stride < row_bytes || destination.size() < rows * stride)
            return Status::error(ErrorCode::invalid_argument, "Invalid benchmark source row range");
        for (std::uint32_t row = 0; row < rows; ++row) {
            if (stop.stop_requested())
                return Status::error(ErrorCode::cancelled, "Benchmark cancelled");
            std::memcpy(destination.data() + row * stride,
                        image_.pixels().data() + (first + row) * image_.row_stride(), row_bytes);
        }
        return {};
    }
    Result<MappedPlane> map_plane(std::uint32_t frame, std::uint32_t plane) const override {
        if (frame != 0 || plane != 0)
            return Status::error(ErrorCode::invalid_argument, "Invalid benchmark source plane");
        return MappedPlane{image_.storage().owner(), image_.pixels(), image_.row_stride()};
    }

  private:
    DocumentDescriptor descriptor_;
    Image image_;
};

class GeneratedRows final : public RasterSource {
  public:
    static constexpr std::uint32_t width = 256;
    static constexpr std::uint32_t height = 131072;
    static constexpr std::size_t row_bytes = width * 4U;

    GeneratedRows() : tile_(row_bytes * period_rows) {
        descriptor_.format = Format::png;
        descriptor_.canvas_width = width;
        descriptor_.canvas_height = height;
        descriptor_.color.primaries = ColorPrimaries::srgb;
        descriptor_.color.transfer = TransferFunction::srgb;
        RasterFrameDescriptor frame;
        frame.width = width;
        frame.height = height;
        frame.color = descriptor_.color;
        frame.layout.planes.push_back({PlaneSemantic::packed, width, height, kRgba8, 8});
        descriptor_.frames.push_back(std::move(frame));
        check(descriptor_.validate());
        // LCM(24,180,25)=1800. Prepare this small repeating tile outside timing;
        // the source never materializes the complete 128 MiB logical raster.
        for (std::uint32_t y = 0; y < period_rows; ++y) {
            for (std::uint32_t x = 0; x < width; ++x) {
                const auto pixel = pattern(x, y, false);
                std::memcpy(tile_.data() + y * row_bytes + x * 4U, pixel.data(), pixel.size());
            }
        }
    }

    const DocumentDescriptor& descriptor() const noexcept override {
        return descriptor_;
    }
    RasterAccess access() const noexcept override {
        return RasterAccess::random_rows | RasterAccess::concurrent_reads;
    }
    Result<void> read_rows(std::uint32_t frame, std::uint32_t plane, std::uint32_t first,
                           std::uint32_t rows, std::size_t stride, std::span<std::byte> destination,
                           std::stop_token stop) const override {
        if (frame != 0 || plane != 0 || first > height || rows > height - first ||
            stride < row_bytes || destination.size() / stride < rows)
            return Status::error(ErrorCode::invalid_argument, "Invalid generated row range");
        for (std::uint32_t row = 0; row < rows; ++row) {
            if (stop.stop_requested())
                return Status::error(ErrorCode::cancelled, "Benchmark cancelled");
            std::memcpy(destination.data() + row * stride,
                        tile_.data() + ((first + row) % period_rows) * row_bytes, row_bytes);
        }
        return {};
    }

  private:
    static constexpr std::uint32_t period_rows = 1800;
    DocumentDescriptor descriptor_;
    std::vector<std::byte> tile_;
};

class VerifyGeneratedRows final : public PixelSink {
  public:
    explicit VerifyGeneratedRows(const GeneratedRows& source) : source_(source) {}
    Result<void> begin(const DocumentInfo& document) override {
        if (document.canvas_width != GeneratedRows::width ||
            document.canvas_height != GeneratedRows::height)
            return Status::error(ErrorCode::invalid_argument, "Tall PNG dimensions changed");
        return {};
    }
    Result<void> begin_frame(std::uint32_t index, const FrameInfo&) override {
        if (index != 0)
            return Status::error(ErrorCode::invalid_argument, "Unexpected tall PNG frame");
        return {};
    }
    Result<void> write_rows(std::uint32_t first, std::uint32_t rows, std::size_t stride,
                            std::span<const std::byte> pixels) override {
        if (first != seen_ || rows > GeneratedRows::height - seen_ ||
            stride < GeneratedRows::row_bytes || pixels.size() / stride < rows)
            return Status::error(ErrorCode::invalid_argument, "Invalid tall PNG decoded rows");
        for (std::uint32_t row = 0; row < rows; ++row) {
            check(source_.read_rows(0, 0, first + row, 1, expected_.size(), expected_, {}));
            if (std::memcmp(expected_.data(), pixels.data() + row * stride, expected_.size()) != 0)
                return Status::error(ErrorCode::invalid_argument, "Tall PNG samples changed");
        }
        seen_ += rows;
        return {};
    }
    Result<void> end_frame(std::uint32_t index) override {
        if (index != 0 || seen_ != GeneratedRows::height)
            return Status::error(ErrorCode::invalid_argument, "Tall PNG frame ended early");
        return {};
    }
    Result<void> end() override {
        return {};
    }

  private:
    const GeneratedRows& source_;
    std::array<std::byte, GeneratedRows::row_bytes> expected_{};
    std::uint32_t seen_ = 0;
};

struct Record {
    std::string name;
    std::uint32_t width;
    std::uint32_t height;
    std::uint32_t output_width;
    std::uint32_t output_height;
    std::vector<double> samples;
    std::uint64_t bytes = 0;
    std::optional<double> psnr;
};

template <class F> std::vector<double> measure(unsigned iterations, F operation) {
    operation(); // One unmeasured warmup, matching xtask.
    std::vector<double> samples;
    for (unsigned iteration = 0; iteration < iterations; ++iteration) {
        const auto start = std::chrono::steady_clock::now();
        operation();
        samples.push_back(
            std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - start)
                .count());
    }
    return samples;
}

std::optional<double> quality(const Document& source, const Document& decoded) {
    const auto reference = source.frames.at(0).image.pixels();
    const auto actual = decoded.frames.at(0).image.pixels();
    if (reference.size() != actual.size())
        throw std::runtime_error("Roundtrip layout mismatch");
    double sum = 0;
    for (std::size_t index = 0; index < reference.size(); ++index) {
        const double delta =
            std::to_integer<int>(reference[index]) - std::to_integer<int>(actual[index]);
        sum += delta * delta;
    }
    if (sum == 0)
        return std::nullopt;
    return 10.0 * std::log10(255.0 * 255.0 * static_cast<double>(reference.size()) / sum);
}

void print_record(Record record) {
    auto sorted = record.samples;
    std::sort(sorted.begin(), sorted.end());
    const auto middle = sorted.size() / 2U;
    const auto median =
        sorted.size() % 2U == 0 ? (sorted[middle - 1U] + sorted[middle]) / 2.0 : sorted[middle];
    std::cout << std::fixed << std::setprecision(6) << "{\"name\":\"" << record.name
              << "\",\"width\":" << record.width << ",\"height\":" << record.height
              << ",\"output_width\":" << record.output_width
              << ",\"output_height\":" << record.output_height << ",\"median_ms\":" << median
              << ",\"output_bytes\":" << record.bytes << ",\"psnr_db\":";
    if (record.psnr)
        std::cout << *record.psnr;
    else
        std::cout << "null";
    std::cout << ",\"samples_ms\":[";
    for (std::size_t index = 0; index < record.samples.size(); ++index) {
        if (index != 0)
            std::cout << ',';
        std::cout << record.samples[index];
    }
    std::cout << "]}" << std::endl;
}

void run(const std::filesystem::path& corpus, const std::filesystem::path& temporary,
         std::uint32_t width, std::uint32_t height, unsigned iterations) {
    Service service;
    for (const auto& name : {std::string{"png"}, std::string{"png-alpha"}, std::string{"jpeg"}}) {
        const bool alpha = name == "png-alpha";
        auto document = fixture(width, height, alpha);
        Source source(document);
        EncodeOptions options;
        options.format = name == "jpeg" ? Format::jpeg : Format::png;
        options.compression_level = 6;
        options.quality = 95;
        options.chroma_subsampling = ChromaSubsampling::yuv444;
        options.progressive = false;
        options.preserve_metadata = false;
        const auto path = temporary / ("legacy-output-" + name + ".bin");
        std::uint64_t bytes = 0;
        auto samples = measure(iterations, [&] {
            auto output = take(file_output(path));
            bytes = take(service.encode(source, output, options)).bytes_written;
        });
        DecodeOptions decode_options;
        decode_options.output_format = kRgba8;
        auto roundtrip = take(service.decode(take(file_input(path)), decode_options));
        auto psnr = quality(document, roundtrip);
        if (name != "jpeg" && psnr)
            throw std::runtime_error("Lossless PNG roundtrip differs");
        print_record({name + "-" + std::to_string(width) + "x" + std::to_string(height), width,
                      height, width, height, std::move(samples), bytes, psnr});
        std::filesystem::remove(path);
    }

    const auto jpeg_path =
        corpus / ("ui-" + std::to_string(width) + "x" + std::to_string(height) + "-jpeg.jpg");
    if (!std::filesystem::exists(jpeg_path)) {
        std::cerr << "Preview corpus missing: " << jpeg_path << '\n';
        return;
    }
    auto input = take(file_input(jpeg_path));
    auto encoded =
        std::make_shared<const std::vector<std::byte>>(take(read_all(*input.source, 256U << 20U)));
    input = memory_input(encoded);
    for (const bool exact : {true, false}) {
        DecodeOptions options;
        options.output_format = kRgba8;
        options.preserve_metadata = false;
        options.orientation = OrientationPolicy::apply;
        const std::uint32_t target_width = std::min(width, 2048U);
        const std::uint32_t target_height = static_cast<std::uint32_t>(
            std::floor(static_cast<double>(height) * target_width / width));
        // libjpeg-turbo supports n/8 IDCT scaling. Choose the smallest available
        // decode at or above exact output, then resize; reduced-only is reported separately.
        const std::uint32_t numerator = (target_width * 8U + width - 1U) / width;
        options.maximum_extent = exact ? (width * numerator + 7U) / 8U : 2048U;
        std::uint32_t output_width = 0;
        std::uint32_t output_height = 0;
        std::uint64_t bytes = 0;
        const auto make_preview = [&] {
            auto decoded = take(service.decode(input, options));
            if (exact &&
                (decoded.canvas_width != target_width || decoded.canvas_height != target_height)) {
                TransformOptions transform_options;
                ResizeOptions resize;
                resize.width = target_width;
                resize.height = target_height;
                resize.method = ResamplingMethod::lanczos3;
                resize.linear_rgb = true;
                resize.premultiply_alpha = true;
                resize.maximum_threads = 8;
                transform_options.resize = resize;
                decoded = take(transform(decoded, transform_options));
            }
            return decoded;
        };
        auto samples = measure(iterations, [&] {
            const auto decoded = make_preview();
            output_width = decoded.canvas_width;
            output_height = decoded.canvas_height;
            bytes = decoded.frames.at(0).image.pixels().size();
        });
        print_record({std::string{exact ? "preview-" : "preview-reduced-only-"} +
                          std::to_string(width) + "x" + std::to_string(height),
                      width, height, output_width, output_height, std::move(samples), bytes,
                      std::nullopt});
        if (exact) {
            // Untimed raw reference for cross-implementation preview quality checks.
            const auto reference = make_preview();
            const auto pixels = reference.frames.at(0).image.pixels();
            std::ofstream output(temporary / ("legacy-preview-" + std::to_string(width) + "x" +
                                              std::to_string(height) + ".rgba"),
                                 std::ios::binary);
            output.write(reinterpret_cast<const char*>(pixels.data()),
                         static_cast<std::streamsize>(pixels.size()));
            if (!output)
                throw std::runtime_error("Could not write legacy preview reference");
        }
    }
}

void run_tall(const std::filesystem::path& temporary, unsigned iterations) {
    Service service;
    GeneratedRows source;
    EncodeOptions options;
    options.format = Format::png;
    options.compression_level = 6;
    options.preserve_metadata = false;
    const auto path = temporary / "legacy-output-tall.png";
    std::uint64_t bytes = 0;
    auto samples = measure(iterations, [&] {
        auto output = take(file_output(path));
        bytes = take(service.encode(source, output, options)).bytes_written;
    });
    // Verify every decoded sample through bounded rows, outside the timed encode.
    VerifyGeneratedRows verifier(source);
    DecodeOptions decode;
    decode.output_format = kRgba8;
    check(service.decode_to_sink(take(file_input(path)), verifier, decode));
    print_record({"tall-png-256x131072", GeneratedRows::width, GeneratedRows::height,
                  GeneratedRows::width, GeneratedRows::height, std::move(samples), bytes,
                  std::nullopt});
    std::filesystem::remove(path);
}

void run_animations(const std::filesystem::path& corpus, unsigned iterations) {
    Service service;
    const auto path = corpus / "four-animations.gif";
    auto file = take(file_input(path));
    auto encoded =
        std::make_shared<const std::vector<std::byte>>(take(read_all(*file.source, 256U << 20U)));
    const auto input = memory_input(encoded);
    std::vector<AnimationDecodeSession> sessions;
    sessions.reserve(4);
    for (unsigned index = 0; index < 4; ++index) {
        auto session = take(service.open_animation_decoder(input));
        const auto& document = session.info().document;
        if (document.canvas_width != 320 || document.canvas_height != 180 ||
            document.frames.size() != 8)
            throw std::runtime_error("Unexpected shared animation benchmark corpus");
        sessions.push_back(std::move(session));
    }
    auto samples = measure(iterations, [&] {
        for (auto& session : sessions)
            check(session.reset());
        for (unsigned frame = 0; frame < 8; ++frame) {
            for (auto& session : sessions) {
                if (!take(session.advance()))
                    throw std::runtime_error("Benchmark animation ended early");
                // Match the public Rust API's independently owned snapshot on every
                // advance. These opaque frames have identical premultiplied samples.
                const auto snapshot = take(session.snapshot_pixels(kPremultipliedRgba8));
                if (snapshot.width() != 320 || snapshot.height() != 180)
                    throw std::runtime_error("Benchmark animation snapshot changed size");
            }
        }
    });
    // Exact corpus verification is deliberately outside the timed workload.
    for (auto& session : sessions) {
        check(session.reset());
        for (unsigned frame = 0; frame < 8; ++frame) {
            if (!take(session.advance()))
                throw std::runtime_error("Benchmark animation ended early");
            const auto snapshot = take(session.snapshot_pixels(kPremultipliedRgba8));
            const auto pixels = snapshot.pixels();
            const std::array<std::byte, 4> expected =
                frame % 2U == 0
                    ? std::array{std::byte{255}, std::byte{0}, std::byte{0}, std::byte{255}}
                    : std::array{std::byte{0}, std::byte{255}, std::byte{0}, std::byte{255}};
            for (std::size_t offset = 0; offset < pixels.size(); offset += 4U) {
                if (std::memcmp(pixels.data() + offset, expected.data(), expected.size()) != 0)
                    throw std::runtime_error("Benchmark animation samples changed");
            }
        }
        if (take(session.advance()))
            throw std::runtime_error("Benchmark animation has unexpected extra frames");
    }
    print_record(
        {"four-animations-32-frames", 320, 180, 320, 180, std::move(samples), 0, std::nullopt});
}
} // namespace

int main(int argc, char** argv) {
    if (argc < 4 || argc > 5) {
        std::cerr << "usage: legacy <corpus-directory> <temporary-directory> <iterations> "
                     "[all|1080|4k|8k|extended]\n";
        return 2;
    }
    try {
        const auto iterations = static_cast<unsigned>(std::stoul(argv[3]));
        if (iterations == 0 || iterations > 1000)
            throw std::runtime_error("Invalid iterations");
        std::filesystem::create_directories(argv[2]);
        const std::string selection = argc == 5 ? argv[4] : "all";
        for (const auto& [name, size] : std::array{std::pair{"1080", std::pair{1920U, 1080U}},
                                                   std::pair{"4k", std::pair{3840U, 2160U}},
                                                   std::pair{"8k", std::pair{7680U, 4320U}}}) {
            if (selection == "all" || selection == name)
                run(argv[1], argv[2], size.first, size.second, iterations);
        }
        if (selection == "all" || selection == "extended") {
            run_tall(argv[2], iterations);
            run_animations(argv[1], iterations);
        }
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
