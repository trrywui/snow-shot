// SPDX-License-Identifier: GPL-3.0-or-later
#import "microphone.h"
#import <AVFAudio/AVFAudio.h>
#import <AudioToolbox/AudioToolbox.h>
#import <Foundation/Foundation.h>
#include <math.h>

// Keep the throwing message send and its @catch in Objective-C. An exception
// must never unwind through a Rust frame, including in panic=abort builds.
@interface SnowMicrophoneCapture : NSObject
@property(nonatomic, strong) AVAudioEngine* engine;
@property(nonatomic, strong) AVAudioInputNode* node;
@property(nonatomic, strong) AVAudioFormat* format;
@property(nonatomic) uint32_t device;
@property(nonatomic) SnowMicrophoneFormat hardwareFormat;
@property(nonatomic) SnowMicrophoneFormat outputFormat;
@property(nonatomic) BOOL tapInstalled;
@property(nonatomic, copy) NSString* operation;
- (BOOL)selectDevice:(void**)error;
- (BOOL)configure:(void**)error;
- (BOOL)start:(AVAudioNodeTapBlock)tap error:(void**)error;
- (void)close:(void**)error;
@end

@implementation SnowMicrophoneCapture
- (void)fail:(void**)error domain:(NSString*)domain code:(NSInteger)code reason:(NSString*)reason {
    NSString* message = [NSString
        stringWithFormat:@"microphone %@: %@; device=%u hardware=%.0fHz/%uch output=%.0fHz/%uch",
                         self.operation, reason, self.device, self.hardwareFormat.sample_rate,
                         self.hardwareFormat.channels, self.outputFormat.sample_rate,
                         self.outputFormat.channels];
    *error =
        (__bridge_retained void*)[NSError errorWithDomain:domain
                                                     code:code
                                                 userInfo:@{NSLocalizedDescriptionKey : message}];
}
- (void)exception:(NSException*)exception error:(void**)error {
    [self fail:error
        domain:exception.name ?: @"snow.microphone.exception"
          code:0
        reason:exception.reason ?: @"no exception reason"];
}
- (BOOL)selectDevice:(void**)error {
    self.operation = @"select-device";
    AudioUnit unit = self.node.audioUnit;
    if (!unit) {
        [self fail:error domain:@"snow.microphone" code:1 reason:@"input AudioUnit is unavailable"];
        return NO;
    }
    AudioDeviceID current = kAudioObjectUnknown;
    UInt32 size = sizeof(current);
    OSStatus status = AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                                           kAudioUnitScope_Global, 0, &current, &size);
    if (status == noErr && current != self.device) {
        AudioDeviceID device = self.device;
        status = AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice,
                                      kAudioUnitScope_Global, 0, &device, sizeof(device));
    }
    if (status != noErr) {
        [self fail:error
            domain:NSOSStatusErrorDomain
              code:status
            reason:@"selecting input device failed"];
        return NO;
    }
    return YES;
}
- (BOOL)configure:(void**)error {
    @try {
        self.operation = @"create-engine";
        if (!self.engine)
            self.engine = [[AVAudioEngine alloc] init];
        self.operation = @"input-node";
        self.node = self.engine.inputNode;
        if (![self selectDevice:error])
            return NO;
        self.operation = @"hardware-format";
        AVAudioFormat* hardware = [self.node inputFormatForBus:0];
        self.hardwareFormat = (SnowMicrophoneFormat){hardware.sampleRate, hardware.channelCount};
        AVAudioFormat* output = [self.node outputFormatForBus:0];
        self.outputFormat = (SnowMicrophoneFormat){output.sampleRate, output.channelCount};
        // The input node cannot resample hardware input. Its cached output
        // format may describe the previous device after a device selection.
        if (!isfinite(hardware.sampleRate) || hardware.sampleRate <= 0 ||
            hardware.channelCount == 0 || hardware.channelCount > 8) {
            [self fail:error
                domain:@"snow.microphone.format"
                  code:1
                reason:@"invalid hardware input format"];
            return NO;
        }
        self.format =
            [[AVAudioFormat alloc] initStandardFormatWithSampleRate:hardware.sampleRate
                                                           channels:hardware.channelCount];
        if (!self.format) {
            [self fail:error
                domain:@"snow.microphone.format"
                  code:2
                reason:@"cannot create Float32 input format"];
            return NO;
        }
        return YES;
    } @catch (NSException* exception) {
        [self exception:exception error:error];
        return NO;
    }
}
- (BOOL)start:(AVAudioNodeTapBlock)tap error:(void**)error {
    @try {
        self.operation = @"install-tap";
        [self.node installTapOnBus:0 bufferSize:1024 format:self.format block:tap];
        self.tapInstalled = YES;
        self.operation = @"prepare-engine";
        [self.engine prepare];
        self.operation = @"start-engine";
        NSError* nativeError = nil;
        if (![self.engine startAndReturnError:&nativeError]) {
            [self fail:error
                domain:nativeError.domain ?: @"snow.microphone"
                  code:nativeError ? nativeError.code : 1
                reason:nativeError.localizedDescription ?: @"starting input engine failed"];
            return NO;
        }
        return YES;
    } @catch (NSException* exception) {
        [self exception:exception error:error];
        return NO;
    }
}
- (void)close:(void**)error {
    // A failed start can leave a tap behind. Retire each acquired resource even
    // if the other cleanup operation throws; preserve the first failure.
    @try {
        self.operation = @"stop-engine";
        [self.engine stop];
    } @catch (NSException* exception) {
        [self exception:exception error:error];
    }
    if (self.tapInstalled) {
        self.tapInstalled = NO;
        @try {
            self.operation = @"remove-tap";
            [self.node removeTapOnBus:0];
        } @catch (NSException* exception) {
            if (!*error)
                [self exception:exception error:error];
        }
    }
}
@end

void* snow_microphone_create(uint32_t device, SnowMicrophoneFormat* format, void** error) {
    @autoreleasepool {
        *error = NULL;
        SnowMicrophoneCapture* capture = [[SnowMicrophoneCapture alloc] init];
        capture.device = device;
        if (![capture configure:error])
            return NULL;
        *format = capture.hardwareFormat;
        return (__bridge_retained void*)capture;
    }
}
bool snow_microphone_start(void* handle, const void* tap, void** error) {
    @autoreleasepool {
        *error = NULL;
        return [(__bridge SnowMicrophoneCapture*)handle start:(__bridge AVAudioNodeTapBlock)tap
                                                        error:error];
    }
}
bool snow_microphone_is_running(void* handle, void** error) {
    @autoreleasepool {
        *error = NULL;
        SnowMicrophoneCapture* capture = (__bridge SnowMicrophoneCapture*)handle;
        @try {
            capture.operation = @"is-running";
            return capture.engine.isRunning;
        } @catch (NSException* exception) {
            [capture exception:exception error:error];
            return false;
        }
    }
}
void snow_microphone_destroy(void* handle, void** error) {
    @autoreleasepool {
        *error = NULL;
        SnowMicrophoneCapture* capture = (__bridge_transfer SnowMicrophoneCapture*)handle;
        [capture close:error];
    }
}
