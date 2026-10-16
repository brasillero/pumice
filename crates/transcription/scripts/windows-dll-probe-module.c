/*
 * Probe module for test-windows-dll-search.ps1. The test stages it as
 * gpu\vulkan\ggml-vulkan.dll in a copy of the bundle, so that ggml loads it
 * after startup, by full path with a plain LoadLibraryW, when it scans the
 * GPU directory in auto mode. It imports pumice_probe from
 * pumice-probe-dep.dll, a name no bundle or system ships: Windows can only
 * find that DLL where the test plants a decoy (the current directory or
 * PATH), so the module loads only if the process-wide DLL search reaches
 * them. Built without the C runtime (/NODEFAULTLIB /ENTRY:DllMain).
 */
#include <windows.h>

__declspec(dllimport) int pumice_probe(void);

/* Referenced from an export so the linker keeps the import. */
__declspec(dllexport) int pumice_probe_module(void) { return pumice_probe(); }

BOOL WINAPI DllMain(HINSTANCE module, DWORD reason, LPVOID reserved) {
    (void)module;
    (void)reason;
    (void)reserved;
    return TRUE;
}
