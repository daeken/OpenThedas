// Adapted from unrar_sys 0.5.8's vendor/unrar/isnt.cpp by Alexander Roshal.
// These checks follow its MinGW implementation, including the GetVersionEx API.
// See docs/mods.md for the UnRAR licence notice.
//
// UnRAR source code may be used in any software to handle RAR archives without limitations
// free of charge, but cannot be used to develop RAR (WinRAR) compatible archiver and to
// re-create RAR compression algorithm, which is proprietary. Distribution of modified UnRAR
// source code in separate form or as a part of other software is permitted, provided that
// full text of this paragraph, starting from "UnRAR source code" words, is included in
// license, or in documentation if license is not available, and in source code comments of
// resulting package.

#include <windows.h>

DWORD WinNT()
{
    OSVERSIONINFOW version = {};
    version.dwOSVersionInfoSize = sizeof(version);
    if (!GetVersionExW(&version) || version.dwPlatformId != VER_PLATFORM_WIN32_NT)
        return 0;
    return version.dwMajorVersion * 0x100 + version.dwMinorVersion;
}

bool IsWindows11OrGreater()
{
    OSVERSIONINFOW version = {};
    version.dwOSVersionInfoSize = sizeof(version);
    return GetVersionExW(&version) &&
           (version.dwMajorVersion > 10 ||
            (version.dwMajorVersion == 10 && version.dwBuildNumber >= 22000));
}
