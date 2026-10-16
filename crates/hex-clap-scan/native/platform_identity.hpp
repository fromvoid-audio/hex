// ORIGINAL platform declaration. Mac admission is the trusted never-fork profile only.
#pragma once
namespace HexFixturePlatform {
#if defined(__APPLE__) && defined(__aarch64__)
constexpr const char* name="macos-arm64";
constexpr bool mac=true;
#elif defined(__linux__) && defined(__x86_64__)
constexpr const char* name="linux-x86_64";
constexpr bool mac=false;
#else
#error unsupported_original_fixture_platform
#endif
}
