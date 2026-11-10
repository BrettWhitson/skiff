//! Operating-system accessibility preferences (Windows; elsewhere the
//! defaults apply). Each is read from the OS at most once per [`REFRESH`]
//! (they are asked for every frame), and again after [`settings_changed`].

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a preference read from the OS is trusted.
pub const REFRESH: Duration = Duration::from_secs(3);

/// A value read at most once per `every`, or again after `invalidate`.
#[derive(Debug)]
pub struct Cached<T> {
    value: Option<(T, Instant)>,
    every: Duration,
}

impl<T: Copy> Cached<T> {
    pub const fn new(every: Duration) -> Self {
        Cached { value: None, every }
    }

    pub fn get(&mut self, now: Instant, read: impl FnOnce() -> T) -> T {
        match self.value {
            Some((v, at)) if now.saturating_duration_since(at) < self.every => v,
            _ => {
                let v = read();
                self.value = Some((v, now));
                v
            }
        }
    }

    pub fn invalidate(&mut self) {
        self.value = None;
    }
}

static HIGH_CONTRAST: Mutex<Cached<bool>> = Mutex::new(Cached::new(REFRESH));
static ANIMATIONS: Mutex<Cached<bool>> = Mutex::new(Cached::new(REFRESH));
static TEXT_SCALE: Mutex<Cached<u32>> = Mutex::new(Cached::new(REFRESH));

fn cached<T: Copy>(c: &Mutex<Cached<T>>, read: fn() -> T) -> T {
    c.lock().unwrap_or_else(|e| e.into_inner()).get(Instant::now(), read)
}

/// The user may have changed OS settings (the window got focus back): the
/// next read asks the OS again.
pub fn settings_changed() {
    for c in [&HIGH_CONTRAST, &ANIMATIONS] {
        c.lock().unwrap_or_else(|e| e.into_inner()).invalidate();
    }
    TEXT_SCALE.lock().unwrap_or_else(|e| e.into_inner()).invalidate();
}

/// Windows' high-contrast mode is on.
pub fn high_contrast() -> bool {
    cached(&HIGH_CONTRAST, read_high_contrast)
}

/// Windows' "Animation effects" setting is on (the default elsewhere).
pub fn animations() -> bool {
    cached(&ANIMATIONS, read_animations)
}

/// Windows' "Text size" (Settings > Accessibility) as a factor, 1 to 2.25.
pub fn text_scale() -> f32 {
    cached(&TEXT_SCALE, read_text_scale).clamp(100, 225) as f32 / 100.0
}

#[cfg(windows)]
fn read_text_scale() -> u32 {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide("Software\\Microsoft\\Accessibility"), wide("TextScaleFactor"));
    let (mut data, mut len) = (100u32, std::mem::size_of::<u32>() as u32);
    // SAFETY: both names are NUL-terminated UTF-16 that outlive the call;
    // RRF_RT_REG_DWORD makes it write at most `len` (4) bytes into `data`.
    let err = unsafe {
        let out = (&mut data as *mut u32).cast();
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), out, &mut len)
    };
    if err == 0 { data } else { 100 } // unset: 100%
}

#[cfg(not(windows))]
fn read_text_scale() -> u32 {
    100
}

#[cfg(windows)]
fn read_high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, dwFlags: 0, lpszDefaultScheme: std::ptr::null_mut() };
    // SAFETY: `hc` is a correctly sized HIGHCONTRASTW that outlives the call;
    // the call only writes into it.
    let ok = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&mut hc as *mut HIGHCONTRASTW).cast(), 0) };
    ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

#[cfg(not(windows))]
fn read_high_contrast() -> bool {
    false
}

#[cfg(windows)]
fn read_animations() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut on: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into `on`.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
    ok == 0 || on != 0
}

#[cfg(not(windows))]
fn read_animations() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_reads_once_per_period_and_after_invalidate() {
        let mut c = Cached::new(Duration::from_secs(3));
        let t0 = Instant::now();
        let reads = std::cell::Cell::new(0);
        let get = |c: &mut Cached<bool>, at| c.get(at, || reads.set(reads.get() + 1) == ());
        get(&mut c, t0);
        get(&mut c, t0 + Duration::from_secs(1));
        assert_eq!(reads.get(), 1, "a frame within the period does not ask the OS");
        get(&mut c, t0 + Duration::from_secs(4));
        assert_eq!(reads.get(), 2);
        c.invalidate();
        get(&mut c, t0 + Duration::from_secs(4));
        assert_eq!(reads.get(), 3, "a settings change asks again");
    }
}
