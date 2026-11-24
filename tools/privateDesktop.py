"""Run a qualification subprocess on a Win32 desktop which is never displayed."""
import ctypes as C
from ctypes import wintypes as W
import subprocess
import time
from uuid import uuid4


class Startup(C.Structure):
    _fields_ = [("cb", W.DWORD), ("reserved", W.LPWSTR), ("desktop", W.LPWSTR), ("title", W.LPWSTR),
                *[(k, W.DWORD) for k in ("x", "y", "width", "height", "cols", "rows", "attr", "flags")],
                ("show", W.WORD), ("reserved_length", W.WORD), ("reserved_data", C.c_void_p),
                ("stdin", W.HANDLE), ("stdout", W.HANDLE), ("stderr", W.HANDLE)]


class ProcessInfo(C.Structure):
    _fields_ = [("process", W.HANDLE), ("thread", W.HANDLE), ("pid", W.DWORD), ("tid", W.DWORD)]


class Limits(C.Structure):
    _fields_ = [("process_time", C.c_int64), ("job_time", C.c_int64), ("flags", W.DWORD),
                ("min_ws", C.c_size_t), ("max_ws", C.c_size_t), ("max_processes", W.DWORD),
                ("affinity", C.c_size_t), ("priority", W.DWORD), ("scheduling", W.DWORD)]


class ExtendedLimits(C.Structure):
    _fields_ = [("basic", Limits), ("io_counters", C.c_uint64 * 6),
                ("memory_limits", C.c_size_t * 4)]


def run(command, timeout):
    u, k = C.windll.user32, C.windll.kernel32
    name = "disqro-qualification-" + uuid4().hex
    u.CreateDesktopW.restype = k.CreateJobObjectW.restype = W.HANDLE
    desk = W.HANDLE(u.CreateDesktopW(name, None, None, 0, 0x01ff, None))
    if not desk:
        raise C.WinError()
    job = W.HANDLE(k.CreateJobObjectW(None, None))
    info = ProcessInfo()
    try:
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not job or not k.SetInformationJobObject(job, 9, C.byref(limits), C.sizeof(limits)):
            raise C.WinError()
        startup = Startup()
        startup.cb, startup.desktop = C.sizeof(startup), name
        args = C.create_unicode_buffer(subprocess.list2cmdline(command))
        if not k.CreateProcessW(None, args, None, None, False, 0x08000004, None, None,
                                C.byref(startup), C.byref(info)):
            raise C.WinError()
        if not k.AssignProcessToJobObject(job, info.process):
            k.TerminateProcess(info.process, 1)
            raise C.WinError()
        if k.ResumeThread(info.thread) == -1:
            raise C.WinError()
        deadline = time.monotonic() + timeout
        while k.WaitForSingleObject(info.process, 500) == 258:
            if time.monotonic() > deadline:
                raise TimeoutError("Silent qualification exceeded its deadline")
        code = W.DWORD()
        if not k.GetExitCodeProcess(info.process, C.byref(code)) or code.value:
            raise RuntimeError(f"Private desktop qualification exited with {code.value}")
    finally:
        for handle in (job, info.thread, info.process):
            if handle:
                k.CloseHandle(handle)
        u.CloseDesktop(desk)
