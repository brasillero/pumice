/*
 * Decoy DLL for test-windows-dll-search.ps1. The test builds one copy per
 * DLL name it plants (bundle DLLs, vulkan-1.dll), each exporting the names
 * of the real DLL so that a static import binds to it. If Windows ever loads
 * a copy, it appends its own path to the file named by PUMICE_DECOY_MARKER
 * and ends the process with exit code 86, so the test cannot miss it.
 * Built without the C runtime (/NODEFAULTLIB /ENTRY:DllMain); every export
 * is decoy_stub.
 */
#include <windows.h>

int decoy_stub(void) { return 0; }

BOOL WINAPI DllMain(HINSTANCE module, DWORD reason, LPVOID reserved) {
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) {
        WCHAR marker[512];
        WCHAR self[512];
        DWORD written;
        DWORD length = GetEnvironmentVariableW(L"PUMICE_DECOY_MARKER", marker, 512);
        if (length > 0 && length < 512) {
            HANDLE file = CreateFileW(marker, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                                      NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
            if (file != INVALID_HANDLE_VALUE) {
                DWORD self_length = GetModuleFileNameW(module, self, 512);
                WriteFile(file, self, self_length * sizeof(WCHAR), &written, NULL);
                WriteFile(file, L"\r\n", 2 * sizeof(WCHAR), &written, NULL);
                CloseHandle(file);
            }
        }
        TerminateProcess(GetCurrentProcess(), 86);
    }
    return TRUE;
}
