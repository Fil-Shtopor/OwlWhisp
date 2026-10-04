"""Check licence conversion through the Windows RichEdit engine used by NSIS."""
import ctypes
import importlib.util
import os
import unittest
from ctypes import wintypes
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("installer_license", ROOT / "scripts/build/installer-license.py")
license_renderer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(license_renderer)


def rich_edit(rtf, inspect):
    """Use an isolated, invisible control; no installed application is opened."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    user = ctypes.WinDLL("user32", use_last_error=True)
    kernel.LoadLibraryW.argtypes = [wintypes.LPCWSTR]
    kernel.LoadLibraryW.restype = wintypes.HMODULE
    module = kernel.LoadLibraryW("msftedit.dll")
    if not module:
        raise ctypes.WinError(ctypes.get_last_error())
    user.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR,
                                    wintypes.DWORD, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                    ctypes.c_int, wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE,
                                    wintypes.LPVOID]
    user.CreateWindowExW.restype = wintypes.HWND
    user.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, ctypes.c_size_t, ctypes.c_ssize_t]
    user.SendMessageW.restype = ctypes.c_ssize_t
    user.DestroyWindow.argtypes = [wintypes.HWND]
    hwnd = user.CreateWindowExW(0, "RICHEDIT50W", "", 0x80000004, 0, 0, 440, 1000, None, None, module, None)
    if not hwnd:
        raise ctypes.WinError(ctypes.get_last_error())
    callback_type = ctypes.WINFUNCTYPE(wintypes.DWORD, ctypes.c_size_t, ctypes.c_void_p,
                                      wintypes.LONG, ctypes.POINTER(wintypes.LONG))
    offset = 0

    @callback_type
    def feed(_cookie, buffer, capacity, count):
        nonlocal offset
        chunk = rtf[offset:offset + capacity]
        ctypes.memmove(buffer, chunk, len(chunk))
        count[0] = len(chunk)
        offset += len(chunk)
        return 0

    class Stream(ctypes.Structure):
        _pack_ = 4  # EDITSTREAM follows RichEdit's four-byte packing on x64 as well.
        _fields_ = [("cookie", ctypes.c_size_t), ("error", wintypes.DWORD), ("callback", callback_type)]

    stream = Stream(0, 0, feed)
    try:
        user.SendMessageW(hwnd, 0x449, 2, ctypes.addressof(stream))  # EM_STREAMIN / SF_RTF
        if stream.error:
            raise ValueError(f"RichEdit could not load licence RTF: {stream.error}")
        return inspect(user, hwnd)
    finally:
        user.DestroyWindow(hwnd)


def text_from_rtf(rtf):
    def inspect(user, hwnd):
        length = user.SendMessageW(hwnd, 14, 0, 0)
        output = ctypes.create_unicode_buffer(length + 2)
        user.SendMessageW(hwnd, 13, len(output), ctypes.addressof(output))
        return output.value
    return rich_edit(rtf, inspect)


@unittest.skipUnless(os.name == "nt", "Requires Windows RichEdit, like the NSIS licence page")
class InstallerLicense(unittest.TestCase):
    def test_unicode_and_table_cells_survive_native_rich_edit(self):
        notice = "# Соглашение — café 👁\n\n| Компонент | Лицензия |\n|---|---|\n| **OwlWhisp** | `Apache-2.0` |\n"
        rtf = license_renderer.render("Full license {braces} \\ text", notice).encode("ascii")
        text = text_from_rtf(rtf)
        for value in ["Соглашение — café 👁", "Компонент", "Лицензия", "OwlWhisp", "Apache-2.0", "Full license {braces} \\ text"]:
            self.assertIn(value, text)
        self.assertNotIn("|---|", text)
        self.assertNotIn("**OwlWhisp**", text)
        self.assertIn("\t", text)  # Real RichEdit table cells, rather than literal Markdown pipes.

    def test_real_licence_retains_complete_apache_terms_and_model_notices(self):
        license_text = (ROOT / "LICENSE").read_text(encoding="utf-8")
        notices = (ROOT / "docs/licenses.md").read_text(encoding="utf-8")
        text = text_from_rtf(license_renderer.render(license_text, notices).encode("ascii"))
        self.assertIn(" ".join(license_text.split()), " ".join(text.split()))
        for value in ["Parakeet", "Qualcomm", "SenseVoice", "MPL-2.0", "Apache-2.0", "Licensing"]:
            self.assertIn(value, text)


if __name__ == "__main__":
    unittest.main()
