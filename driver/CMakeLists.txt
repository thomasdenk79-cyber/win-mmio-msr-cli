cmake_minimum_required(VERSION 3.25)
project(rapl_pkg_driver C)
# The KMDF/WDK toolchain supplies ntddk.h, wdf.h and linker settings.
# Configure from an x64 Native Tools + WDK prompt; never build this with MinGW.
add_compile_definitions(RAPL_ENABLE_LIVE_WRITES=0)
add_library(rapl-pkg SHARED src/driver.c)
target_include_directories(rapl-pkg PRIVATE include)
