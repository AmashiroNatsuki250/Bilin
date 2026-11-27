/* Compile as an ordinary Windows executable linked against Kernel32 and Advapi32.
 * The IO mocks exercise the actual static helper without loading the kernel driver. */
#define UNICODE
#include <assert.h>
#include <winsock2.h>
#include <windows.h>

enum test_result
{
    IMMEDIATE_SUCCESS,
    IMMEDIATE_FAILURE,
    PENDING_SUCCESS,
    PENDING_FAILURE
};

static enum test_result io_result;
static HANDLE io_event;
static BOOL completion_waited;

static BOOL WINAPI TestDeviceIoControl(HANDLE handle, DWORD code,
    LPVOID in, DWORD in_len, LPVOID out, DWORD out_len, LPDWORD bytes,
    LPOVERLAPPED overlapped)
{
    DWORD flags;
    io_event = overlapped->hEvent;
    assert(GetHandleInformation(io_event, &flags));
    if (io_result == IMMEDIATE_SUCCESS)
    {
        *bytes = 17;
        return TRUE;
    }
    SetLastError(io_result == IMMEDIATE_FAILURE ? ERROR_INVALID_FUNCTION : ERROR_IO_PENDING);
    return FALSE;
}

static BOOL WINAPI TestGetOverlappedResult(HANDLE file,
    LPOVERLAPPED overlapped, LPDWORD bytes, BOOL wait)
{
    DWORD flags;
    assert(wait);
    assert(overlapped->hEvent == io_event);
    assert(GetHandleInformation(io_event, &flags));
    completion_waited = TRUE;
    if (io_result == PENDING_FAILURE)
    {
        SetLastError(ERROR_OPERATION_ABORTED);
        return FALSE;
    }
    *bytes = 17;
    return TRUE;
}

#define WINDIVERT_STATIC
#define DeviceIoControl TestDeviceIoControl
#define GetOverlappedResult TestGetOverlappedResult
#include "../dll/windivert.c"
#undef DeviceIoControl
#undef GetOverlappedResult

static void TestCompletion(enum test_result result, BOOL expected_success,
    BOOL expected_wait, DWORD expected_error)
{
    DWORD flags;
    UINT bytes = 0;
    WINDIVERT_IOCTL request = {0};
    BOOL success;
    DWORD error;

    io_result = result;
    io_event = NULL;
    completion_waited = FALSE;
    success = WinDivertIoControl(NULL, 0, &request, NULL, 0, &bytes);
    error = GetLastError();
    assert(success == expected_success);
    assert(completion_waited == expected_wait);
    assert(io_event != NULL);
    assert(!GetHandleInformation(io_event, &flags));
    if (success)
    {
        assert(bytes == 17);
    }
    else
    {
        assert(error == expected_error);
        assert(bytes == 0);
    }
}

int main(void)
{
    TestCompletion(IMMEDIATE_SUCCESS, TRUE, FALSE, ERROR_SUCCESS);
    TestCompletion(IMMEDIATE_FAILURE, FALSE, FALSE, ERROR_INVALID_FUNCTION);
    TestCompletion(PENDING_SUCCESS, TRUE, TRUE, ERROR_SUCCESS);
    TestCompletion(PENDING_FAILURE, FALSE, TRUE, ERROR_OPERATION_ABORTED);
    return 0;
}
