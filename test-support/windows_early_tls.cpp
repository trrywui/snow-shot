// SPDX-License-Identifier: Apache-2.0
#include "windows_early_tls.h"

#include <Windows.h>

#include <cstdlib>
#include <new>

namespace {
bool initializingEarlyTls = false;
bool initializedEarlyTls = false;
} // namespace

void* operator new(std::size_t size) {
    if (initializingEarlyTls) {
        constexpr char message[] = "FAIL: heap allocation during pre-CRT TLS initialization.\n";
        DWORD written = 0;
        WriteFile(GetStdHandle(STD_ERROR_HANDLE), message, static_cast<DWORD>(sizeof(message) - 1),
                  &written, nullptr);
        ExitProcess(81);
    }
    if (void* memory = std::malloc(size == 0 ? 1 : size))
        return memory;
    throw std::bad_alloc();
}

void operator delete(void* memory) noexcept {
    std::free(memory);
}

void operator delete(void* memory, std::size_t) noexcept {
    std::free(memory);
}

extern "C" void WINAPI __dyn_tls_init(void*, DWORD, void*) noexcept;

namespace {
void NTAPI initializeTlsBeforeCrt(void*, DWORD reason, void*) {
    if (reason != DLL_PROCESS_ATTACH)
        return;
    initializingEarlyTls = true;
    __dyn_tls_init(nullptr, DLL_THREAD_ATTACH, nullptr);
    initializingEarlyTls = false;
    initializedEarlyTls = true;
}
} // namespace

// Exercise early TLS without installing injected DLLs or changing services.
#pragma section(".CRT$XLB", long, read)
extern "C" __declspec(allocate(".CRT$XLB")) const PIMAGE_TLS_CALLBACK snowTestEarlyTlsCallback =
    initializeTlsBeforeCrt;
#pragma comment(linker, "/include:snowTestEarlyTlsCallback")

bool snow_test::early_tls::initializedBeforeCrt() {
    return initializedEarlyTls;
}
