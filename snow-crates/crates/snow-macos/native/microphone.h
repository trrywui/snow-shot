// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <stdbool.h>
#include <stdint.h>

typedef struct SnowMicrophoneFormat {
    double sample_rate;
    uint32_t channels;
} SnowMicrophoneFormat;

// Handles and errors are retained Objective-C objects. Each error is transferred
// to the caller, which must release it. The tap is copied by AVAudioEngine.
void* snow_microphone_create(uint32_t device, SnowMicrophoneFormat* format, void** error);
bool snow_microphone_start(void* capture, const void* tap, void** error);
bool snow_microphone_is_running(void* capture, void** error);
void snow_microphone_destroy(void* capture, void** error);
