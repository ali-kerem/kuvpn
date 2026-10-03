use console::{Style, Term};
use dialoguer::{Input, Password};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Trait for providing credentials and user input.
///
/// `request_text` / `request_password` return `None` when the prompt was
/// dismissed externally (e.g. the page changed while the user was typing).
pub trait CredentialsProvider: Send + Sync {
    fn request_text(&self, msg: &str) -> Option<String>;
    fn request_password(&self, msg: &str) -> Option<String>;
    /// Request the user's email/username. Defaults to `request_text`.
    /// Implementations may override this to distinguish the email prompt from
    /// generic text prompts (e.g. to auto-save the entered address).
    fn request_email(&self, msg: &str) -> Option<String> {
        self.request_text(msg)
    }
    fn on_mfa_push(&self, _code: &str) {}
    fn on_mfa_complete(&self) {}

    /// Install a guard that is polled while a prompt is visible.
    /// Return `false` → page changed → prompt is dismissed and `request_*` returns `None`.
    fn set_page_guard(&self, _guard: Box<dyn Fn() -> bool + Send + Sync>) {}

    /// Remove any active page guard.
    fn clear_page_guard(&self) {}
}

/// A implementation of `CredentialsProvider` for terminal input.
pub struct TerminalCredentialsProvider;

impl CredentialsProvider for TerminalCredentialsProvider {
    fn request_text(&self, msg: &str) -> Option<String> {
        // Clear the current line in case a spinner is active on another thread
        let term = Term::stderr();
        let _ = term.clear_line();
        Some(
            Input::new()
                .with_prompt(msg.trim_end_matches(": ").trim_end_matches(':'))
                .interact_text()
                .unwrap_or_default(),
        )
    }

    fn request_password(&self, msg: &str) -> Option<String> {
        // Clear the current line in case a spinner is active on another thread
        let term = Term::stderr();
        let _ = term.clear_line();
        Some(
            Password::new()
                .with_prompt(msg.trim_end_matches(": ").trim_end_matches(':'))
                .interact()
                .unwrap_or_default(),
        )
    }

    fn on_mfa_push(&self, code: &str) {
        let bold = Style::new().bold();
        let cyan = Style::new().cyan().bold();
        eprintln!();
        eprintln!(
            "{} Enter {} in Authenticator",
            bold.apply_to(">>"),
            cyan.apply_to(code),
        );
        eprintln!();
    }

    fn on_mfa_complete(&self) {
        let green = Style::new().green();
        eprintln!("  {} MFA approved", green.apply_to("✓"));
    }
}

use fd_lock::{RwLock, RwLockWriteGuard};
use once_cell::sync::Lazy;
use std::fs::File;
use std::sync::Mutex;

// Stores the write guard to keep the lock held for the process lifetime.
static INSTANCE_LOCK: Lazy<Mutex<Option<RwLockWriteGuard<'static, File>>>> =
    Lazy::new(|| Mutex::new(None));

/// Platform-relative path from the home directory to the kuvpn profile directory.
#[cfg(target_os = "linux")]
const PROFILE_SUBPATH: &str = ".local/share/kuvpn/profile";

#[cfg(target_os = "macos")]
const PROFILE_SUBPATH: &str = "Library/Application Support/kuvpn/profile";

#[cfg(target_os = "windows")]
const PROFILE_SUBPATH: &str = "AppData/Roaming/kuvpn/profile";

/// Returns the profile directory path without creating it.
fn profile_dir_path() -> Result<PathBuf, Box<dyn Error>> {
    let home_dir = env::var("HOME").or_else(|_| env::var("USERPROFILE"))?;
    Ok(PathBuf::from(&home_dir).join(PROFILE_SUBPATH))
}

/// Returns the kuvpn application data directory (`…/kuvpn/`), creating it if absent.
/// This is the parent of the profile directory and contains history, lock files, etc.
pub(crate) fn kuvpn_data_dir() -> Result<PathBuf, Box<dyn Error>> {
    let mut dir = profile_dir_path()?;
    dir.pop(); // …/kuvpn/profile → …/kuvpn/
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Ensures that only one instance of the application is running.
/// Returns an error if another instance is already active.
pub fn ensure_single_instance() -> Result<(), Box<dyn Error>> {
    let lock_path = kuvpn_data_dir()?.join("kuvpn.lock");

    let file = File::create(&lock_path)?;
    // Leak the RwLock to get a 'static reference so the guard can outlive this function.
    let lock_ref: &'static mut RwLock<File> = Box::leak(Box::new(RwLock::new(file)));

    match lock_ref.try_write() {
        Ok(guard) => {
            *INSTANCE_LOCK.lock().unwrap() = Some(guard);
            Ok(())
        }
        Err(_) => Err("Another instance of KUVPN is already running.".into()),
    }
}

/// Returns a platform-appropriate user data directory for the Chrome profile.
///
/// - **Linux:** `~/.local/share/kuvpn/profile`
/// - **macOS:** `~/Library/Application Support/kuvpn/profile`
/// - **Windows:** `%USERPROFILE%\AppData\Roaming\kuvpn\profile`
///
/// Creates the directory if it does not already exist.
pub fn get_user_data_dir() -> Result<PathBuf, Box<dyn Error>> {
    let user_data_dir = profile_dir_path()?;
    if !user_data_dir.exists() {
        std::fs::create_dir_all(&user_data_dir)?;
        log::info!("User data directory created at: {:?}", user_data_dir);
    }
    Ok(user_data_dir)
}

/// Returns `true` if the profile directory exists and contains at least one entry,
/// indicating that browser session data has been saved. App settings/logs do not count.
/// Does NOT create the directory.
pub fn has_session_data() -> bool {
    profile_dir_path()
        .ok()
        .and_then(|p| std::fs::read_dir(p).ok())
        .is_some_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| !is_app_data_file(&entry.path()))
        })
}

fn is_app_data_file(path: &std::path::Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            "gui_settings.json" | "kuvpn.log" | "kuvpn.log.1" | "kuvpn-gui.log" | "kuvpn-gui.log.1"
        )
    )
}

/// Removes browser session data, preserving KUVPN settings and diagnostic logs.
/// Does not create the directory first.
/// Uses path-component checks to guard against wiping unrelated directories.
pub fn wipe_user_data_dir() -> Result<(), Box<dyn Error>> {
    let path = profile_dir_path()?;
    if !path.components().any(|c| c.as_os_str() == "kuvpn") {
        return Err("Refusing to wipe: path does not appear to be a kuvpn directory".into());
    }
    if path.exists() {
        clear_browser_profile(&path)?;
        log::info!("Cleared browser session in: {:?}", path);
    }
    Ok(())
}

fn clear_browser_profile(path: &std::path::Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if is_app_data_file(&entry.path()) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            // Remove symlinks themselves, not their targets (Chrome singleton files).
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Escapes JavaScript strings to prevent injection.
pub fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProfile(PathBuf);
    impl Drop for TestProfile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn clearing_session_preserves_settings_and_open_log_files() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let profile = TestProfile(
            std::env::temp_dir()
                .join(format!("kuvpn-profile-test-{}-{nonce}", std::process::id(),)),
        );
        std::fs::create_dir(&profile.0).unwrap();
        std::fs::create_dir(profile.0.join("Default")).unwrap();
        std::fs::write(profile.0.join("Default/Cookies"), "test session").unwrap();
        std::fs::write(profile.0.join("Local State"), "browser state").unwrap();
        std::fs::write(profile.0.join("gui_settings.json"), "saved settings").unwrap();
        std::fs::write(profile.0.join("kuvpn-gui.log.1"), "previous log").unwrap();
        let log_path = profile.0.join("kuvpn-gui.log");
        let mut logger = crate::FileLogger::open(log_path.clone()).unwrap();
        logger.write_line("before clearing session");

        clear_browser_profile(&profile.0).unwrap();
        logger.write_line("after clearing session");
        assert!(!profile.0.join("Default").exists());
        assert!(!profile.0.join("Local State").exists());
        assert_eq!(
            std::fs::read_to_string(profile.0.join("gui_settings.json")).unwrap(),
            "saved settings"
        );
        assert!(profile.0.join("kuvpn-gui.log.1").exists());
        let log = std::fs::read_to_string(log_path).unwrap();
        assert!(log.contains("before clearing session"));
        assert!(log.contains("after clearing session"));
        assert!(std::fs::read_dir(&profile.0)
            .unwrap()
            .all(|entry| is_app_data_file(&entry.unwrap().path())));
        // A repeated cleanup is safe and still preserves the app files.
        clear_browser_profile(&profile.0).unwrap();
    }
}
