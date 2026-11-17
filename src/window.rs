//! `--topmost`: keeps the terminal window that hosts winmon above other windows.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindowLongW, SetWindowPos, GA_ROOTOWNER, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOPMOST,
};

const FLAGS: u32 = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;

/// Makes the terminal window topmost while alive and restores it on drop.
pub struct TopMost {
    hwnd: HWND,
}

impl TopMost {
    /// `None` if there is no console window, it was already topmost (left untouched), or the call failed.
    pub fn enable() -> Option<Self> {
        unsafe {
            let console = GetConsoleWindow();
            if console.is_null() {
                return None;
            }
            // Under Windows Terminal the console window is a hidden pseudo window owned by the real
            // terminal window; with classic conhost the root owner is the console window itself.
            let root = GetAncestor(console, GA_ROOTOWNER);
            let hwnd = if root.is_null() { console } else { root };
            if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 {
                return None;
            }
            (SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, FLAGS) != 0).then_some(Self { hwnd })
        }
    }
}

impl Drop for TopMost {
    fn drop(&mut self) {
        unsafe {
            SetWindowPos(self.hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, FLAGS);
        }
    }
}
