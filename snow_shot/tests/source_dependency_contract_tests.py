"""Configure-only checks for source-owned, edition-specific link requirements."""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "cmake/SnowShotSourceDependencies.cmake"
MINI_CONTRACT = ROOT / "cmake/SnowShotMiniBuildContract.cmake"
CMAKE = shutil.which("cmake")


@unittest.skipUnless(CMAKE, "CMake is required")
class SourceDependencyContractTests(unittest.TestCase):
    def configure(self, fixture, succeeds=True):
        with tempfile.TemporaryDirectory(prefix="snow source contracts ") as directory:
            source = Path(directory)
            for relative in ("owned/consumer.cpp", "unrelated/consumer.cpp", "native.cpp"):
                path = source / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("// Configure-only source fixture.\n", encoding="utf-8")
            (source / "CMakeLists.txt").write_text(
                "cmake_minimum_required(VERSION 3.30)\n"
                "project(SourceDependencyContract LANGUAGES NONE)\n"
                f'include("{MODULE.as_posix()}")\n'
                + fixture,
                encoding="utf-8")
            result = subprocess.run(
                [CMAKE, "-S", str(source), "-B", str(source / "build")],
                capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=30)
            self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)
            return result.stdout + result.stderr

    def test_late_providers_and_clones_keep_source_ownership_and_edition(self):
        self.configure(f'''
get_property(targets DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
if(targets)
    message(FATAL_ERROR "Including source contracts must not create build targets")
endif()
add_library(original INTERFACE owned/consumer.cpp native.cpp)
get_target_property(copied_sources original SOURCES)
add_library(clone INTERFACE ${{copied_sources}})
snow_shot_set_source_dependency_edition(clone mini)
add_library(absolute_clone INTERFACE
    "${{CMAKE_CURRENT_SOURCE_DIR}}/owned/consumer.cpp")
snow_shot_set_source_dependency_edition(absolute_clone mini)
# The unrelated file shares its basename; ownership must use the actual source.
add_library(unrelated INTERFACE unrelated/consumer.cpp)
# Naming a target mini does not select an edition provider.
add_library(mini_named_full_consumer INTERFACE owned/consumer.cpp)
snow_shot_source_requires(print SOURCES ./owned/consumer.cpp)
snow_shot_source_requires(native_surface SOURCES native.cpp)
snow_shot_register_source_capability(print
    FULL_TARGET snow_shot_print MINI_TARGET snow_shot_print_mini)
snow_shot_register_source_capability(native_surface
    FULL_TARGET platform_surface MINI_TARGET platform_surface)
add_library(snow_shot_print INTERFACE)
add_library(snow_shot_print_mini INTERFACE)
add_library(platform_surface INTERFACE)
get_target_property(early_links original INTERFACE_LINK_LIBRARIES)
if(early_links)
    message(FATAL_ERROR "Registering ownership must not mutate consumers before resolution")
endif()
snow_shot_resolve_source_dependencies()
snow_shot_resolve_source_dependencies()
function(expect_links target expected)
    get_target_property(actual ${{target}} INTERFACE_LINK_LIBRARIES)
    if(NOT actual STREQUAL expected)
        message(FATAL_ERROR "${{target}} links '${{actual}}', expected '${{expected}}'")
    endif()
endfunction()
expect_links(original "snow_shot_print;platform_surface")
expect_links(clone "snow_shot_print_mini;platform_surface")
expect_links(absolute_clone "snow_shot_print_mini")
expect_links(mini_named_full_consumer "snow_shot_print")
get_target_property(unrelated_links unrelated INTERFACE_LINK_LIBRARIES)
if(unrelated_links)
    message(FATAL_ERROR "A matching basename must not acquire another source's dependencies")
endif()
include("{MINI_CONTRACT.as_posix()}")
snow_shot_assert_mini_build_contract(clone)
snow_shot_assert_mini_build_contract(absolute_clone)
''')

    def test_sources_and_consumers_can_be_declared_after_providers(self):
        self.configure('''
add_library(provider INTERFACE)
snow_shot_register_source_capability(print FULL_TARGET provider)
snow_shot_source_requires(print SOURCES owned/consumer.cpp)
add_library(consumer INTERFACE owned/consumer.cpp)
target_link_libraries(consumer INTERFACE provider)
snow_shot_resolve_source_dependencies()
get_target_property(links consumer INTERFACE_LINK_LIBRARIES)
if(NOT links STREQUAL "provider")
    message(FATAL_ERROR "Explicit provider links must remain present exactly once")
endif()
''')

    def test_interface_sources_export_their_provider(self):
        self.configure('''
add_library(provider INTERFACE)
snow_shot_register_source_capability(print FULL_TARGET provider)
snow_shot_source_requires(print SOURCES owned/consumer.cpp)
add_library(consumer INTERFACE)
target_sources(consumer INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}/owned/consumer.cpp")
snow_shot_resolve_source_dependencies()
get_target_property(links consumer INTERFACE_LINK_LIBRARIES)
if(NOT links STREQUAL "provider")
    message(FATAL_ERROR "Interface sources must export their implementation provider")
endif()
''')

    def test_unknown_capability_fails_at_resolution(self):
        output = self.configure('''
add_library(consumer INTERFACE owned/consumer.cpp)
snow_shot_source_requires(undeclared SOURCES owned/consumer.cpp)
snow_shot_resolve_source_dependencies()
''', succeeds=False)
        self.assertIn("requires unregistered source capability 'undeclared'", output)

    def test_missing_provider_target_fails_at_resolution(self):
        output = self.configure('''
add_library(consumer INTERFACE owned/consumer.cpp)
snow_shot_source_requires(print SOURCES owned/consumer.cpp)
snow_shot_register_source_capability(print FULL_TARGET missing_print)
snow_shot_resolve_source_dependencies()
''', succeeds=False)
        self.assertIn("requires missing provider target 'missing_print'", output)

    def test_mini_never_falls_back_to_a_full_provider(self):
        output = self.configure('''
add_library(full_print INTERFACE)
add_library(consumer INTERFACE owned/consumer.cpp)
snow_shot_set_source_dependency_edition(consumer mini)
snow_shot_source_requires(print SOURCES owned/consumer.cpp)
snow_shot_register_source_capability(print FULL_TARGET full_print)
snow_shot_resolve_source_dependencies()
''', succeeds=False)
        self.assertIn("has no mini provider for 'consumer'", output)

    def test_consumers_without_requirements_do_not_need_a_provider(self):
        self.configure('''
snow_shot_register_source_capability(print FULL_TARGET absent_full MINI_TARGET absent_mini)
snow_shot_source_requires(print SOURCES owned/consumer.cpp)
add_library(unrelated INTERFACE unrelated/consumer.cpp)
snow_shot_resolve_source_dependencies()
get_target_property(links unrelated INTERFACE_LINK_LIBRARIES)
if(links)
    message(FATAL_ERROR "Unused capabilities must not add dependencies")
endif()
''')


if __name__ == "__main__":
    unittest.main()
