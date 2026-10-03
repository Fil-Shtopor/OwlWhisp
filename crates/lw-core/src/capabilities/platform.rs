//! Explicit target platform, shared by detection, provider policy and runtime lookup.

use serde::{Deserialize, Serialize};

/// Operating systems supported by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingSystem {
    /// Microsoft Windows.
    Windows,
    /// Apple macOS.
    Macos,
    /// Linux.
    Linux,
    /// An unrecognized operating system.
    Unknown,
}

/// Native process architecture (not the architecture of an emulated host).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    /// 64-bit Intel/AMD.
    X64,
    /// 64-bit ARM.
    Arm64,
    /// An unrecognized or unsupported architecture.
    Unknown,
}

/// Platform inputs can be supplied explicitly so every policy is testable on any host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    /// Operating system.
    pub os: OperatingSystem,
    /// Architecture of the executable.
    pub arch: Architecture,
}

impl Platform {
    /// Normalize OS/architecture spellings used by Rust and package metadata.
    pub fn from_names(os: &str, arch: &str) -> Self {
        Self {
            os: match os.to_ascii_lowercase().as_str() {
                "windows" | "win" => OperatingSystem::Windows,
                "macos" | "darwin" | "osx" => OperatingSystem::Macos,
                "linux" => OperatingSystem::Linux,
                _ => OperatingSystem::Unknown,
            },
            arch: match arch.to_ascii_lowercase().as_str() {
                "x86_64" | "x64" | "amd64" => Architecture::X64,
                "aarch64" | "arm64" => Architecture::Arm64,
                _ => Architecture::Unknown,
            },
        }
    }

    /// Platform this executable was built for.
    pub fn current() -> Self {
        Self::from_names(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Rust's canonical operating-system name.
    pub fn os_name(self) -> &'static str {
        match self.os {
            OperatingSystem::Windows => "windows",
            OperatingSystem::Macos => "macos",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Unknown => "unknown",
        }
    }

    /// Rust's canonical architecture name.
    pub fn arch_name(self) -> &'static str {
        match self.arch {
            Architecture::X64 => "x86_64",
            Architecture::Arm64 => "aarch64",
            Architecture::Unknown => "unknown",
        }
    }

    /// Runtime package directory. Unknown targets must never silently select x64 packages.
    pub fn runtime_dir(self) -> Option<&'static str> {
        use Architecture::{Arm64, X64};
        use OperatingSystem::{Linux, Macos, Windows};
        match (self.os, self.arch) {
            (Windows, X64) => Some("win-x64"),
            (Windows, Arm64) => Some("win-arm64"),
            (Macos, X64) => Some("osx-x64"),
            (Macos, Arm64) => Some("osx-arm64"),
            (Linux, X64) => Some("linux-x64"),
            (Linux, Arm64) => Some("linux-arm64"),
            _ => None,
        }
    }

    /// ONNX Runtime library name, independent of the host running a test.
    pub fn runtime_library(self) -> Option<&'static str> {
        match self.os {
            OperatingSystem::Windows => Some("onnxruntime.dll"),
            OperatingSystem::Macos => Some("libonnxruntime.dylib"),
            OperatingSystem::Linux => Some("libonnxruntime.so"),
            OperatingSystem::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_aliases_and_unknown_targets() {
        for os in ["macos", "darwin", "OSX"] {
            for arch in ["aarch64", "ARM64"] {
                assert_eq!(Platform::from_names(os, arch).runtime_dir(), Some("osx-arm64"));
            }
        }
        for arch in ["x64", "AMD64", "x86_64"] {
            assert_eq!(Platform::from_names("WIN", arch).runtime_dir(), Some("win-x64"));
        }
        for (os, arch) in [("windows", "x86"), ("linux", "armv7"), ("freebsd", "x64")] {
            assert_eq!(Platform::from_names(os, arch).runtime_dir(), None);
        }
    }
}
