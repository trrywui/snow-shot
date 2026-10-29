// SPDX-License-Identifier: Apache-2.0
#pragma once

namespace snow_test::early_tls {
// Link windows_early_tls.cpp once into a Windows startup fixture. It initializes
// dynamic TLS before CRT startup and rejects heap allocations in that interval.
bool initializedBeforeCrt();
} // namespace snow_test::early_tls
