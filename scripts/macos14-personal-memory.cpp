// Standalone read-only sampler; never linked into the screenshot application.
#include <libproc.h>
#include <mach/mach_time.h>
#include <sys/resource.h>
#include <unistd.h>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <thread>
#include <vector>

namespace {
struct Process {
    int pid;
    rusage_info_v4 previous{};
    bool sampled = false;
};
int positiveNumber(const char* value) {
    char* end = nullptr;
    const long number = std::strtol(value, &end, 10);
    if (!end || *end || number < 1 || number > 10000000)
        return -1;
    return static_cast<int>(number);
}
} // namespace

int main(int argc, char** argv) {
    if (argc < 3) {
        std::fprintf(stderr, "Usage: sampler SECONDS PID [PID ...]\n");
        return 2;
    }
    const int seconds = positiveNumber(argv[1]);
    if (seconds < 1 || seconds > 3600)
        return 2;
    std::vector<Process> processes;
    for (int index = 2; index < argc; ++index) {
        const int pid = positiveNumber(argv[index]);
        if (pid < 1)
            return 2;
        processes.push_back({pid, {}, false});
    }
    using Clock = std::chrono::steady_clock;
    const auto started = Clock::now();
    auto previousTime = started;
    mach_timebase_info_data_t timebase{};
    if (mach_timebase_info(&timebase) != KERN_SUCCESS || !timebase.denom)
        return 1;
    const double nanosecondsPerTick = static_cast<double>(timebase.numer) / timebase.denom;
    std::puts("elapsed_seconds,pid,physical_footprint_bytes,resident_bytes,cpu_percent,interrupt_"
              "wakeups,package_idle_wakeups");
    for (int index = 0; index <= seconds; ++index) {
        const auto now = Clock::now();
        const double elapsed = std::chrono::duration<double>(now - started).count();
        const double interval = std::chrono::duration<double>(now - previousTime).count();
        for (auto& process : processes) {
            rusage_info_v4 current{};
            if (proc_pid_rusage(process.pid, RUSAGE_INFO_V4,
                                reinterpret_cast<rusage_info_t*>(&current)) != 0) {
                std::fprintf(stderr, "Cannot sample PID %d: %s\n", process.pid,
                             std::strerror(errno));
                return 1;
            }
            double cpu = 0;
            unsigned long long interrupt = 0;
            unsigned long long idle = 0;
            if (process.sampled) {
                if (current.ri_proc_start_abstime != process.previous.ri_proc_start_abstime) {
                    std::fprintf(stderr, "PID %d was reused; start a new measurement\n",
                                 process.pid);
                    return 1;
                }
                // Rusage CPU counters use Mach ticks; Intel's 1:1 timebase must
                // not be assumed. 100% means one fully occupied core.
                cpu = static_cast<double>(current.ri_user_time + current.ri_system_time -
                                          process.previous.ri_user_time -
                                          process.previous.ri_system_time) /
                      (interval * 1e9) * nanosecondsPerTick * 100;
                interrupt = current.ri_interrupt_wkups - process.previous.ri_interrupt_wkups;
                idle = current.ri_pkg_idle_wkups - process.previous.ri_pkg_idle_wkups;
            }
            std::printf("%.3f,%d,%llu,%llu,%.4f,%llu,%llu\n", elapsed, process.pid,
                        static_cast<unsigned long long>(current.ri_phys_footprint),
                        static_cast<unsigned long long>(current.ri_resident_size), cpu, interrupt,
                        idle);
            process.previous = current;
            process.sampled = true;
        }
        std::fflush(stdout);
        previousTime = now;
        if (index != seconds)
            std::this_thread::sleep_until(started + std::chrono::seconds(index + 1));
    }
}
