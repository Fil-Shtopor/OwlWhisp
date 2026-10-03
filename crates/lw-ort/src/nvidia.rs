//! Query NVIDIA's installed driver without depending on downloaded CUDA libraries or GPU names.
use std::ffi::{CStr, c_char, c_int};

/// A physical NVIDIA device as reported by the CUDA driver API.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NvidiaDevice {
    /// CUDA ordinal, also used by the execution provider.
    pub ordinal: i32,
    /// Driver-reported device name.
    pub name: String,
    /// Unique GPU identity, independent of the marketing name.
    pub uuid: String,
    /// Compute capability major version.
    pub major: i32,
    /// Compute capability minor version.
    pub minor: i32,
    /// Maximum CUDA API supported by the installed driver (e.g. 13000).
    pub driver_cuda_version: i32,
}

impl NvidiaDevice {
    /// The pinned Windows TensorRT 10.14 resource for this exact architecture.
    pub fn tensorrt_resource(&self) -> Option<String> {
        resource_sm(self.major, self.minor).map(|sm| format!("nvinfer_builder_resource_sm{sm}_10"))
    }
}

/// Architectures with a verified, published Windows resource in the add-on catalog.
pub fn resource_sm(major: i32, minor: i32) -> Option<i32> {
    let sm = major * 10 + minor;
    [75, 80, 86, 89, 90, 120].contains(&sm).then_some(sm)
}

/// Driver ordinal 0 is also the device used by OwlWhisp's CUDA/TensorRT sessions.
pub fn devices() -> Result<Vec<NvidiaDevice>, String> {
    #[cfg(windows)]
    let path = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("System32/nvcuda.dll");
    #[cfg(not(windows))]
    let path = std::path::PathBuf::from("libcuda.so.1");
    // CUDA's stable driver ABI; every symbol stays borrowed while the library is loaded.
    unsafe {
        let lib = libloading::Library::new(&path).map_err(|e| format!("NVIDIA driver: {e}"))?;
        let init = lib
            .get::<unsafe extern "system" fn(u32) -> c_int>(b"cuInit\0")
            .map_err(|e| e.to_string())?;
        let count = lib
            .get::<unsafe extern "system" fn(*mut c_int) -> c_int>(b"cuDeviceGetCount\0")
            .map_err(|e| e.to_string())?;
        let device = lib
            .get::<unsafe extern "system" fn(*mut c_int, c_int) -> c_int>(b"cuDeviceGet\0")
            .map_err(|e| e.to_string())?;
        let name = lib
            .get::<unsafe extern "system" fn(*mut c_char, c_int, c_int) -> c_int>(b"cuDeviceGetName\0")
            .map_err(|e| e.to_string())?;
        let capability = lib
            .get::<unsafe extern "system" fn(*mut c_int, *mut c_int, c_int) -> c_int>(
                b"cuDeviceComputeCapability\0",
            )
            .map_err(|e| e.to_string())?;
        let version = lib
            .get::<unsafe extern "system" fn(*mut c_int) -> c_int>(b"cuDriverGetVersion\0")
            .map_err(|e| e.to_string())?;
        let uuid = lib
            .get::<unsafe extern "system" fn(*mut [u8; 16], c_int) -> c_int>(b"cuDeviceGetUuid_v2\0")
            .or_else(|_| lib.get(b"cuDeviceGetUuid\0"))
            .map_err(|e| e.to_string())?;
        let check = |status| {
            if status == 0 {
                Ok(())
            } else {
                Err(format!("NVIDIA driver returned CUDA error {status}"))
            }
        };
        check(init(0))?;
        let mut n = 0;
        let mut driver = 0;
        check(count(&mut n))?;
        check(version(&mut driver))?;
        let mut out = Vec::new();
        for ordinal in 0..n {
            let (mut dev, mut major, mut minor) = (0, 0, 0);
            let mut bytes = [0u8; 16];
            let mut label = [0 as c_char; 256];
            check(device(&mut dev, ordinal))?;
            check(name(label.as_mut_ptr(), label.len() as c_int, dev))?;
            check(capability(&mut major, &mut minor, dev))?;
            check(uuid(&mut bytes, dev))?;
            out.push(NvidiaDevice {
                ordinal,
                name: CStr::from_ptr(label.as_ptr()).to_string_lossy().into_owned(),
                uuid: hex::encode(bytes),
                major,
                minor,
                driver_cuda_version: driver,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_follow_compute_architecture_not_marketing_name() {
        for (major, minor, sm) in [
            (7, 5, 75),
            (8, 0, 80),
            (8, 6, 86),
            (8, 9, 89),
            (9, 0, 90),
            (12, 0, 120),
        ] {
            assert_eq!(resource_sm(major, minor), Some(sm));
        }
        assert_eq!(resource_sm(6, 1), None);
        assert_eq!(resource_sm(12, 1), None);
        assert_eq!(resource_sm(10, 0), None);
    }
}
