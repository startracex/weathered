//! Operating system and CPU architecture as Mojang's version JSON names them.

/// Operating system names used in library and argument rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsName {
    /// Windows.
    Windows,
    /// macOS. Mojang's JSON calls this `osx`.
    Osx,
    /// Linux.
    Linux,
}

impl OsName {
    /// Name written in version JSON rules.
    pub fn mojang(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Osx => "osx",
            Self::Linux => "linux",
        }
    }

    /// Operating system this process is running on.
    ///
    /// Unrecognized Unix systems are treated as Linux so library rules still
    /// select a classpath. Mojang only publishes Windows, macOS, and Linux metadata.
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => Self::Windows,
            "macos" => Self::Osx,
            _ => Self::Linux,
        }
    }
}

/// CPU architecture, reduced to the set Mojang distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 32-bit x86.
    X86,
    /// 64-bit x86.
    X86_64,
    /// 32-bit ARM.
    Arm32,
    /// 64-bit ARM, including Apple Silicon.
    Arm64,
    /// An architecture Mojang's rules do not name.
    Other,
}

impl Arch {
    /// Architecture of this process.
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "x86" | "i686" => Self::X86,
            "x86_64" => Self::X86_64,
            "aarch64" => Self::Arm64,
            "arm" => Self::Arm32,
            _ => Self::Other,
        }
    }

    /// Whether a rule's `os.arch` string refers to this architecture.
    ///
    /// `x86` is 32-bit only. A 64-bit machine does not match it.
    pub fn matches_mojang(self, rule: &str) -> bool {
        let rule = rule.to_ascii_lowercase();
        match self {
            Self::X86 => matches!(rule.as_str(), "x86" | "i386" | "i686" | "x32" | "32"),
            Self::X86_64 => matches!(rule.as_str(), "x86_64" | "amd64" | "x64" | "64"),
            Self::Arm64 => matches!(rule.as_str(), "arm64" | "aarch64"),
            Self::Arm32 => matches!(rule.as_str(), "arm32" | "arm"),
            Self::Other => false,
        }
    }
}

/// The OS a launch or install should target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsInfo {
    /// Operating system.
    pub name: OsName,
    /// CPU architecture.
    pub arch: Arch,
    /// Version string compared against `os.version` regular expressions.
    pub version: String,
}

impl OsInfo {
    /// Build an OS description. Tests use this to target Windows from Linux.
    pub fn new(name: OsName, arch: Arch, version: impl Into<String>) -> Self {
        Self {
            name,
            arch,
            version: version.into(),
        }
    }

    /// The machine the process is running on.
    pub fn current() -> Self {
        Self {
            name: OsName::current(),
            arch: Arch::current(),
            version: read_os_version(),
        }
    }

    /// Classpath separator used by this operating system.
    pub fn classpath_separator(&self) -> &'static str {
        if self.name == OsName::Windows {
            ";"
        } else {
            ":"
        }
    }
}

/// Pull a `10.0.19045` style version out of `cmd /C ver` output.
pub fn extract_windows_version(text: &str) -> Option<String> {
    let marker = "Version ";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let version = rest[..end].trim_end_matches('.');
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

#[cfg(target_os = "windows")]
fn read_os_version() -> String {
    let Ok(output) = std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output()
    else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    extract_windows_version(&text).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn read_os_version() -> String {
    let Ok(output) = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[cfg(target_os = "linux")]
fn read_os_version() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn read_os_version() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classpath_separator_follows_the_named_os() {
        let linux = OsInfo::new(OsName::Linux, Arch::X86_64, "");
        let windows = OsInfo::new(OsName::Windows, Arch::X86_64, "10.0.19045");
        assert_eq!(linux.classpath_separator(), ":");
        assert_eq!(windows.classpath_separator(), ";");
    }

    #[test]
    fn x86_rule_does_not_match_x86_64() {
        assert!(!Arch::X86_64.matches_mojang("x86"));
        assert!(Arch::X86.matches_mojang("x86"));
        assert!(Arch::X86_64.matches_mojang("x64"));
        assert!(Arch::Arm64.matches_mojang("aarch64"));
        assert!(Arch::Arm64.matches_mojang("arm64"));
    }

    #[test]
    fn parses_windows_ver_output() {
        let text = "\r\nMicrosoft Windows [Version 10.0.19045.3803]\r\n";
        assert_eq!(
            extract_windows_version(text).as_deref(),
            Some("10.0.19045.3803")
        );
    }

    #[test]
    fn current_os_is_recognized() {
        let os = OsInfo::current();
        assert!(!os.classpath_separator().is_empty());
        let _ = os.name.mojang();
    }
}
