// SPDX-License-Identifier: GPL-3.0-or-later
// Compile the production boundary unchanged; inject fake native collaborators
// so actual NSExceptions exercise its catch/cleanup paths without permissions.
#import "../native/microphone.m"
#include <stdio.h>
#include <stdlib.h>

static void require(BOOL condition, const char* message) {
    if (!condition) {
        fprintf(stderr, "%s\n", message);
        exit(1);
    }
}
static void raiseFailure(void) {
    @throw [NSException exceptionWithName:@"com.apple.coreaudio.avfaudio"
                                   reason:@"required condition is false: hardware format — 音频"
                                 userInfo:nil];
}

@interface FakeFormat : NSObject
@property(nonatomic) double sampleRate;
@property(nonatomic) AVAudioChannelCount channelCount;
@end
@implementation FakeFormat
@end

@interface FakeInput : NSObject
@property(nonatomic, strong) AVAudioFormat* hardware;
@property(nonatomic, strong) AVAudioFormat* output;
@property(nonatomic, strong) AVAudioFormat* installed;
@property(nonatomic) BOOL selected;
@property(nonatomic) BOOL failFormat;
@property(nonatomic) BOOL failTap;
@property(nonatomic) BOOL failRemove;
@property(nonatomic) NSUInteger installs;
@property(nonatomic) NSUInteger removals;
@end
@implementation FakeInput
- (AVAudioFormat*)inputFormatForBus:(NSUInteger)bus {
    require(bus == 0 && self.selected, "hardware format must be read after device selection");
    if (self.failFormat)
        raiseFailure();
    return self.hardware;
}
- (AVAudioFormat*)outputFormatForBus:(NSUInteger)bus {
    require(bus == 0, "unexpected output bus");
    return self.output;
}
- (void)installTapOnBus:(NSUInteger)bus
             bufferSize:(AVAudioFrameCount)size
                 format:(AVAudioFormat*)format
                  block:(AVAudioNodeTapBlock)block {
    require(bus == 0 && size == 1024 && block != nil, "unexpected tap arguments");
    ++self.installs;
    if (self.failTap)
        raiseFailure();
    self.installed = format;
}
- (void)removeTapOnBus:(NSUInteger)bus {
    require(bus == 0, "unexpected removal bus");
    ++self.removals;
    if (self.failRemove)
        raiseFailure();
}
@end

@interface FakeEngine : NSObject
@property(nonatomic, strong) FakeInput* input;
@property(nonatomic) BOOL failInput;
@property(nonatomic) BOOL failPrepare;
@property(nonatomic) BOOL failStart;
@property(nonatomic) BOOL returnStartError;
@property(nonatomic) BOOL failStop;
@property(nonatomic) NSUInteger prepares;
@property(nonatomic) NSUInteger starts;
@property(nonatomic) NSUInteger stops;
@end
@implementation FakeEngine
- (AVAudioInputNode*)inputNode {
    if (self.failInput)
        raiseFailure();
    return (AVAudioInputNode*)self.input;
}
- (void)prepare {
    ++self.prepares;
    if (self.failPrepare)
        raiseFailure();
}
- (BOOL)startAndReturnError:(NSError**)error {
    ++self.starts;
    if (self.failStart)
        raiseFailure();
    if (self.returnStartError) {
        *error = [NSError errorWithDomain:@"native.start"
                                     code:17
                                 userInfo:@{NSLocalizedDescriptionKey : @"device disconnected"}];
        return NO;
    }
    return YES;
}
- (void)stop {
    ++self.stops;
    if (self.failStop)
        raiseFailure();
}
- (BOOL)isRunning {
    return self.starts > 0 && self.stops == 0;
}
@end

@interface FixtureCapture : SnowMicrophoneCapture
@end
@implementation FixtureCapture
- (BOOL)selectDevice:(void**)error {
    (void)error;
    ((FakeInput*)self.node).selected = YES;
    return YES;
}
@end

static FixtureCapture* fixture(void) {
    FakeInput* input = [FakeInput new];
    input.hardware = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:48000 channels:1];
    // Reproduce an output format cached before the device was selected.
    input.output = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:44100 channels:2];
    FakeEngine* engine = [FakeEngine new];
    engine.input = input;
    FixtureCapture* capture = [FixtureCapture new];
    capture.device = 123;
    capture.engine = (AVAudioEngine*)engine;
    return capture;
}
static NSError* takeError(void* error) {
    require(error != NULL, "failure must include a retained error");
    return (__bridge_transfer NSError*)error;
}
static void checkException(NSError* error, NSString* operation) {
    require([error.domain isEqualToString:@"com.apple.coreaudio.avfaudio"], "exception name lost");
    require([error.localizedDescription containsString:@"hardware format — 音频"],
            "exception reason lost");
    require([error.localizedDescription containsString:operation], "failed operation lost");
    require([error.localizedDescription containsString:@"device=123"], "device ID lost");
}
static AVAudioNodeTapBlock tap(void) {
    return ^(AVAudioPCMBuffer* buffer, AVAudioTime* time) {
      (void)buffer;
      (void)time;
    };
}

int snow_microphone_regression(void) {
    @autoreleasepool {
        void* error = NULL;
        FixtureCapture* capture = fixture();
        FakeEngine* engine = (FakeEngine*)capture.engine;
        require([capture configure:&error], "valid hardware should configure");
        require(error == NULL, "success should have no error");
        require([capture start:tap() error:&error], "valid hardware should start");
        require(engine.input.installed.sampleRate == 48000 &&
                    engine.input.installed.channelCount == 1,
                "tap must match hardware, not cached output format");
        require(engine.input.installed.commonFormat == AVAudioPCMFormatFloat32 &&
                    !engine.input.installed.interleaved,
                "tap must deliver planar Float32");
        [capture close:&error];
        require(engine.stops == 1 && engine.input.removals == 1,
                "successful session must retire its tap");
        [capture close:&error];
        require(engine.input.removals == 1, "tap must not be removed twice");

        // A missing/disabled input must fail before installing a tap, even if
        // the output still has a valid format.
        const SnowMicrophoneFormat invalidFormats[] = {{0, 1},  {48000, 0}, {48000, 9},
                                                       {-1, 1}, {NAN, 1},   {INFINITY, 1}};
        for (size_t i = 0; i < sizeof(invalidFormats) / sizeof(invalidFormats[0]); ++i) {
            capture = fixture();
            engine = (FakeEngine*)capture.engine;
            FakeFormat* hardware = [FakeFormat new];
            hardware.sampleRate = invalidFormats[i].sample_rate;
            hardware.channelCount = invalidFormats[i].channels;
            engine.input.hardware = (AVAudioFormat*)hardware;
            require(![capture configure:&error], "invalid hardware must be rejected");
            require([takeError(error).domain isEqualToString:@"snow.microphone.format"],
                    "invalid format needs classification");
            require(engine.input.installs == 0, "invalid hardware must not install a tap");
            error = NULL;
            [capture close:&error];
            require(engine.input.removals == 0, "uninstalled tap must not be removed");
        }

        for (NSString* operation in @[
                 @"input-node", @"hardware-format", @"install-tap", @"prepare-engine",
                 @"start-engine"
             ]) {
            capture = fixture();
            engine = (FakeEngine*)capture.engine;
            engine.failInput = [operation isEqualToString:@"input-node"];
            engine.input.failFormat = [operation isEqualToString:@"hardware-format"];
            engine.input.failTap = [operation isEqualToString:@"install-tap"];
            engine.failPrepare = [operation isEqualToString:@"prepare-engine"];
            engine.failStart = [operation isEqualToString:@"start-engine"];
            error = NULL;
            BOOL configured = [capture configure:&error];
            BOOL started = configured && [capture start:tap() error:&error];
            require(!started, "native exception must return failure");
            checkException(takeError(error), operation);
            error = NULL;
            [capture close:&error];
            require(error == NULL, "cleanup after failed setup should succeed");
            BOOL installed = [operation isEqualToString:@"prepare-engine"] ||
                             [operation isEqualToString:@"start-engine"];
            require(engine.input.removals == (installed ? 1u : 0u),
                    "partial setup cleanup must track tap ownership");
            require(!engine.failPrepare || engine.starts == 0,
                    "prepare failure must not start engine");
            require(!engine.input.failTap || engine.prepares == 0,
                    "tap failure must not prepare engine");
        }

        capture = fixture();
        engine = (FakeEngine*)capture.engine;
        require([capture configure:&error], "configuration failed");
        engine.returnStartError = YES;
        require(![capture start:tap() error:&error], "NSError start failure must propagate");
        NSError* native = takeError(error);
        require([native.domain isEqualToString:@"native.start"] && native.code == 17 &&
                    [native.localizedDescription containsString:@"device disconnected"],
                "NSError details lost");
        error = NULL;
        [capture close:&error];
        require(engine.input.removals == 1, "NSError start failure must retire tap");

        capture = fixture();
        engine = (FakeEngine*)capture.engine;
        require([capture configure:&error] && [capture start:tap() error:&error],
                "configuration failed");
        engine.failStop = YES;
        engine.input.failRemove = YES;
        [capture close:&error];
        checkException(takeError(error), @"stop-engine");
        require(engine.input.removals == 1, "stop exception must not skip removing the tap");

        capture = fixture();
        engine = (FakeEngine*)capture.engine;
        error = NULL;
        require([capture configure:&error] && [capture start:tap() error:&error],
                "configuration failed");
        engine.input.failRemove = YES;
        [capture close:&error];
        checkException(takeError(error), @"remove-tap");

        // Drain fixture autoreleases before checking ownership transferred
        // through the exported C boundary.
        __weak FixtureCapture* weakCapture;
        @autoreleasepool {
            capture = fixture();
            engine = (FakeEngine*)capture.engine;
            error = NULL;
            require([capture configure:&error], "configuration failed");
            engine.input.failTap = YES;
            weakCapture = capture;
            void* handle = (__bridge_retained void*)capture;
            capture = nil;
            require(!snow_microphone_start(handle, (__bridge const void*)tap(), &error),
                    "C start must contain exception");
            checkException(takeError(error), @"install-tap");
            snow_microphone_destroy(handle, &error);
            require(error == NULL && engine.input.removals == 0,
                    "C destroy must release failed startup safely");
        }
        require(weakCapture == nil, "C destroy must release native capture ownership");
    }
    return 0;
}
