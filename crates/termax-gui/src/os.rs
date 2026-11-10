//! Operating-system accessibility preferences (Windows; elsewhere the
//! defaults apply).

/// Windows' high-contrast mode is on.
#[cfg(windows)]
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: `hc` is a correctly sized HIGHCONTRASTW that outlives the call;
    // the call only writes into it.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&mut hc as *mut HIGHCONTRASTW).cast(), 0) };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

#[cfg(not(windows))]
pub fn high_contrast() -> bool {
    false
}
