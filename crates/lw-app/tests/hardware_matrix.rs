//! Simulated OS/driver/provider observations. No native runtime, GPU, network or model required.
use lw_app::accelerators::{
    AcceleratorAction as Action, AcceleratorReadiness, ModelAvailability as Status, model_availability_on,
};
use lw_app::runtime_install::{SetupAction, package_ids_on, setup_action_on};
use lw_core::capabilities::{ALL_ACCELERATORS, Accelerator as A, OperatingSystem, Platform};
use lw_ort::nvidia::NvidiaDevice;
use lw_platform::caps::{
    CapabilityObservations, capabilities_from_observations, detect_npu_in, linux_gpu_descriptions,
    npu_info_from_arch,
};
use serde::Deserialize;
use std::collections::{BTreeSet, HashSet};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Machine {
    name: String,
    os: String,
    arch: String,
    cpu: String,
    registry: Option<String>,
    cores: Option<usize>,
    ram_bytes: Option<u64>,
    npu_driver_arch: Option<u32>,
    gpus: Vec<String>,
    #[serde(default)]
    nvidia: Vec<(String, i32, i32, i32)>,
    expect_hardware: Vec<A>,
    expect_npu_arch: Option<u32>,
    expect_generation: Option<String>,
    expect_cuda_action: Option<String>,
    expect_trt_action: Option<String>,
    expect_trt_sm: Option<i32>,
}

impl Machine {
    fn platform(&self) -> Platform {
        Platform::from_names(&self.os, &self.arch)
    }
    fn devices(&self) -> Vec<NvidiaDevice> {
        self.nvidia
            .iter()
            .enumerate()
            .map(|(ordinal, (name, major, minor, driver))| NvidiaDevice {
                ordinal: ordinal as i32,
                name: name.clone(),
                uuid: format!("fixture-{}-{ordinal}", self.name),
                major: *major,
                minor: *minor,
                driver_cuda_version: *driver,
            })
            .collect()
    }
    fn hardware_descriptions(&self) -> Vec<String> {
        lw_app::diagnostics::hardware_device_descriptions(&[], &self.gpus, &self.devices())
    }
    fn capabilities(&self) -> lw_core::capabilities::Capabilities {
        capabilities_from_observations(CapabilityObservations {
            platform: self.platform(),
            os_version: "fixture OS version".into(),
            cpu_brand: self.cpu.clone(),
            registry_cpu_brand: self.registry.clone(),
            cpu_cores: self.cores.unwrap_or(8),
            ram_bytes: self.ram_bytes.unwrap_or(16 * 1024 * 1024 * 1024),
            qnn_driver: self
                .npu_driver_arch
                .map(|arch| npu_info_from_arch(arch, Some("1.0.1.1".into())))
                .unwrap_or_default(),
        })
    }
    fn expected_setup(&self, accel: A) -> Option<SetupAction> {
        let value = match accel {
            A::Cuda => self.expect_cuda_action.as_deref(),
            A::TensorRt => self.expect_trt_action.as_deref(),
            A::DirectMl | A::WebGpu if self.os == "windows" && self.expect_hardware.contains(&accel) => {
                Some("download_runtime")
            }
            _ => None,
        };
        match value {
            None => None,
            Some("download_runtime") => Some(SetupAction::DownloadRuntime),
            Some("install_driver") => Some(SetupAction::InstallDriver),
            Some(other) => panic!("{}: invalid expected action {other}", self.name),
        }
    }
}

fn machines() -> Vec<Machine> {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/hardware/configurations.json"
    ))
    .unwrap()
}

fn ready() -> AcceleratorReadiness {
    AcceleratorReadiness {
        parakeet: true,
        model_ready: true,
        gpu_model_ready: true,
        npu_model_ready: true,
        probe_ok: true,
        hardware_present: true,
        provider_registered: true,
        usable: true,
        ..Default::default()
    }
}

#[test]
fn fixtures_cover_all_six_platforms_and_pinned_nvidia_architectures() {
    let machines = machines();
    assert!(machines.len() >= 40);
    let names: HashSet<_> = machines.iter().map(|m| &m.name).collect();
    assert_eq!(names.len(), machines.len(), "fixture names must be unique");
    for os in ["windows", "macos", "linux"] {
        for arch in ["x64", "arm64"] {
            assert!(
                machines.iter().any(|m| m.os == os && m.arch == arch),
                "missing {os}/{arch}"
            );
        }
    }
    let partitions: BTreeSet<_> = machines.iter().filter_map(|m| m.expect_trt_sm).collect();
    assert_eq!(partitions, BTreeSet::from([75, 80, 86, 89, 90, 120]));
}

#[test]
fn provider_support_library_names_and_runtime_directories_for_every_target() {
    // Explicit expectations: this is provider support, not a promise of a bundled/downloadable EP.
    let targets: &[(&str, &str, Option<&str>, &[A])] = &[
        (
            "windows",
            "x64",
            Some("win-x64"),
            &[
                A::Cpu,
                A::WebGpu,
                A::Cuda,
                A::TensorRt,
                A::DirectMl,
                A::OpenVino,
                A::VitisAi,
            ],
        ),
        (
            "windows",
            "arm64",
            Some("win-arm64"),
            &[A::Cpu, A::QnnNpu, A::WebGpu, A::DirectMl],
        ),
        ("macos", "x64", Some("osx-x64"), &[A::Cpu, A::CoreMl, A::WebGpu]),
        (
            "macos",
            "arm64",
            Some("osx-arm64"),
            &[A::Cpu, A::CoreMl, A::WebGpu],
        ),
        (
            "linux",
            "x64",
            Some("linux-x64"),
            &[A::Cpu, A::WebGpu, A::Cuda, A::TensorRt, A::OpenVino],
        ),
        (
            "linux",
            "arm64",
            Some("linux-arm64"),
            &[A::Cpu, A::WebGpu, A::Cuda, A::TensorRt],
        ),
        ("windows", "x86", None, &[A::Cpu]),
        ("linux", "armv7", None, &[A::Cpu]),
        ("freebsd", "x64", None, &[A::Cpu]),
    ];
    for &(os, arch, directory, supported) in targets {
        let platform = Platform::from_names(os, arch);
        assert_eq!(platform.runtime_dir(), directory, "{os}/{arch}");
        for accel in ALL_ACCELERATORS {
            let expected = supported.contains(&accel);
            assert_eq!(accel.supported_on(platform), expected, "{os}/{arch}/{accel:?}");
            let library = accel.library_file_on(platform);
            assert_eq!(
                library.is_some(),
                expected && !matches!(accel, A::Cpu | A::DirectMl),
                "{os}/{arch}/{accel:?}: {library:?}"
            );
            if let Some(name) = library {
                let suffix = match os {
                    "windows" => ".dll",
                    "macos" => ".dylib",
                    "linux" => ".so",
                    _ => unreachable!(),
                };
                assert!(name.ends_with(suffix), "{os}/{arch}/{accel:?}: {name}");
                let stem = match accel {
                    A::QnnNpu => "qnn",
                    A::WebGpu => "webgpu",
                    A::Cuda => "cuda",
                    A::TensorRt => "tensorrt",
                    A::CoreMl => "coreml",
                    A::OpenVino => "openvino",
                    A::VitisAi => "vitisai",
                    A::Cpu | A::DirectMl => unreachable!(),
                };
                let prefix = if os == "windows" { "" } else { "lib" };
                assert_eq!(
                    name,
                    format!("{prefix}onnxruntime_providers_{stem}{suffix}"),
                    "{os}/{arch}/{accel:?}"
                );
            }
        }
    }
    for (os, name) in [
        ("windows", "onnxruntime.dll"),
        ("macos", "libonnxruntime.dylib"),
        ("linux", "libonnxruntime.so"),
    ] {
        for arch in ["x64", "arm64"] {
            assert_eq!(Platform::from_names(os, arch).runtime_library(), Some(name));
        }
    }
    assert_eq!(Platform::from_names("freebsd", "x64").runtime_library(), None);
    for accel in ALL_ACCELERATORS {
        assert_eq!(
            accel.supported_on_this_platform(),
            accel.supported_on(Platform::current())
        );
        assert_eq!(accel.library_file(), accel.library_file_on(Platform::current()));
    }
    assert_eq!(
        lw_ort::onnxruntime_lib_name(),
        Platform::current().runtime_library().unwrap()
    );
}

#[test]
fn simulated_os_observations_detect_cpu_gpu_and_npu_without_host_state() {
    for m in machines() {
        let caps = m.capabilities();
        let platform = m.platform();
        assert_eq!(caps.os, platform.os_name(), "{}", m.name);
        assert_eq!(caps.arch, platform.arch_name(), "{}", m.name);
        assert_eq!(caps.os_version, "fixture OS version");
        assert_eq!(caps.cpu_cores, m.cores.unwrap_or(8).max(1), "{}", m.name);
        assert_eq!(
            caps.ram_mib,
            m.ram_bytes.unwrap_or(16 * 1024 * 1024 * 1024) / (1024 * 1024),
            "{}",
            m.name
        );
        assert_eq!(caps.snapdragon_generation, m.expect_generation, "{}", m.name);
        assert_eq!(
            caps.npu.present,
            m.expect_npu_arch.is_some(),
            "{}: stale driver cannot create an NPU",
            m.name
        );
        assert_eq!(
            caps.npu.htp_arch.map(|a| a.num()),
            m.expect_npu_arch,
            "{}",
            m.name
        );
        assert!(caps.providers.cpu);
        assert!(
            !caps.providers.qnn && !caps.providers.coreml && !caps.providers.directml,
            "{}: hardware cannot imply a loaded provider",
            m.name
        );
        for accel in ALL_ACCELERATORS {
            assert_eq!(
                accel.relevant_to_hardware_on(platform, &caps.cpu_brand, &m.hardware_descriptions()),
                m.expect_hardware.contains(&accel),
                "{} / {accel:?}: {:?}",
                m.name,
                m.gpus
            );
        }
    }
}

#[test]
fn cpu_registry_precedence_and_missing_observations_are_platform_specific() {
    for (os, brand, registry, expected) in [
        (
            "windows",
            "ARM Processor",
            Some(" Snapdragon X2 Elite "),
            "Snapdragon X2 Elite",
        ),
        ("windows", " Intel Core i7 ", None, "Intel Core i7"),
        ("windows", " Intel Core i7 ", Some("  "), "Intel Core i7"),
        ("macos", " Apple M4 ", Some("Qualcomm Snapdragon X2"), "Apple M4"),
        (
            "linux",
            " Ampere Altra ",
            Some("Qualcomm Snapdragon X2"),
            "Ampere Altra",
        ),
        ("windows", "", None, ""),
    ] {
        let caps = capabilities_from_observations(CapabilityObservations {
            platform: Platform::from_names(os, "arm64"),
            os_version: String::new(),
            cpu_brand: brand.into(),
            registry_cpu_brand: registry.map(String::from),
            cpu_cores: 0,
            ram_bytes: 1024 * 1024 + 1023,
            qnn_driver: Default::default(),
        });
        assert_eq!(caps.cpu_brand, expected, "{os}/{brand}");
        assert_eq!(caps.cpu_cores, 1);
        assert_eq!(caps.ram_mib, 1);
    }
}

#[test]
fn setup_actions_and_exact_package_sets_match_the_device_and_driver() {
    let common = ["cuda-core", "cudart", "cublas", "cufft", "curand", "cudnn"];
    for m in machines() {
        let devices = m.devices();
        for accel in ALL_ACCELERATORS {
            let hardware = m.expect_hardware.contains(&accel);
            let action = setup_action_on(m.platform(), accel, hardware, false, &devices);
            assert_eq!(action, m.expected_setup(accel), "{} / {accel:?}", m.name);
            assert_eq!(
                setup_action_on(m.platform(), accel, hardware, true, &devices),
                None,
                "{}: no installation when already usable",
                m.name
            );
            assert_eq!(
                setup_action_on(m.platform(), accel, false, false, &devices),
                None,
                "{}: never offer packages for absent hardware",
                m.name
            );
            if action == Some(SetupAction::DownloadRuntime) {
                let ids = package_ids_on(m.platform(), accel, &devices).unwrap();
                let mut expected: Vec<String> = match accel {
                    A::Cuda | A::TensorRt => common.iter().map(|s| (*s).into()).collect(),
                    A::DirectMl => vec![
                        if m.arch == "arm64" {
                            "directml-arm64"
                        } else {
                            "directml"
                        }
                        .into(),
                    ],
                    A::WebGpu => vec![
                        if m.arch == "arm64" {
                            "webgpu-arm64"
                        } else {
                            "webgpu"
                        }
                        .into(),
                    ],
                    _ => panic!("unexpected downloadable {accel:?}"),
                };
                if accel == A::TensorRt {
                    expected.extend(["nvinfer_10", "nvinfer_plugin_10", "nvonnxparser_10"].map(String::from));
                    expected.push(format!(
                        "nvinfer_builder_resource_sm{}_10",
                        m.expect_trt_sm.unwrap()
                    ));
                }
                assert_eq!(
                    ids, expected,
                    "{} / {accel:?}: wrong architecture or unneeded package",
                    m.name
                );
            } else if matches!(accel, A::Cuda | A::TensorRt) {
                assert!(
                    package_ids_on(m.platform(), accel, &devices).is_err(),
                    "{} / {accel:?}: unsupported package plan",
                    m.name
                );
            }
        }
        if m.platform().os != OperatingSystem::Windows {
            for accel in ALL_ACCELERATORS {
                assert!(
                    package_ids_on(m.platform(), accel, &devices).is_err(),
                    "{}: x64 DLL downloads on another target",
                    m.name
                );
            }
        }
    }
}

#[test]
fn rtx_spark_preview_uses_arm64_gpu_packages_and_never_snapdragon_qnn_or_x64_cuda() {
    let platform = Platform::from_names("windows", "arm64");
    let devices = vec!["NVIDIA RTX Spark Blackwell GPU VEN_10DE".into()];
    let note = lw_app::accelerators::experimental_gpu_note_on(platform, "NVIDIA N1X", &devices).unwrap();
    assert!(note.contains("not tested on a physical RTX Spark"));
    assert!(!A::QnnNpu.relevant_to_hardware_on(platform, "NVIDIA N1X", &devices));
    for accel in [A::DirectMl, A::WebGpu] {
        assert!(accel.relevant_to_hardware_on(platform, "NVIDIA N1X", &devices));
        let plan = package_ids_on(platform, accel, &[]).unwrap();
        assert_eq!(plan.len(), 1);
        assert!(plan[0].ends_with("-arm64"));
        assert_eq!(
            model_availability_on(platform, accel, &ready()),
            (Status::Available, None)
        );
    }
    for accel in [A::Cuda, A::TensorRt] {
        assert!(!accel.supported_on(platform));
        assert!(package_ids_on(platform, accel, &[]).is_err());
    }
    assert!(
        lw_app::accelerators::experimental_gpu_note_on(
            platform,
            "Snapdragon X2 Elite",
            &["Qualcomm Adreno GPU".into()]
        )
        .is_none()
    );
    assert!(
        lw_app::accelerators::experimental_gpu_note_on(
            Platform::from_names("windows", "x64"),
            "Intel Core i9",
            &devices
        )
        .is_none()
    );
}

#[test]
fn settings_status_uses_hardware_provider_packages_and_model_together() {
    for m in machines() {
        let caps = m.capabilities();
        let devices = m.devices();
        for accel in ALL_ACCELERATORS {
            let hardware =
                accel.relevant_to_hardware_on(m.platform(), &caps.cpu_brand, &m.hardware_descriptions());
            let setup = setup_action_on(m.platform(), accel, hardware, false, &devices);
            let mut r = ready();
            r.hardware_present = hardware;
            r.npu_model_ready = caps.npu.present;
            r.provider_registered = accel == A::Cpu;
            r.usable = accel == A::Cpu;
            r.runtime_installable = setup == Some(SetupAction::DownloadRuntime);
            r.driver_needed = setup == Some(SetupAction::InstallDriver);
            let expected = if accel == A::Cpu {
                (Status::Available, None)
            } else {
                match setup {
                    Some(SetupAction::DownloadRuntime) => {
                        (Status::NeedAdditionalAction, Some(Action::DownloadRuntime))
                    }
                    Some(SetupAction::InstallDriver) => {
                        (Status::NeedAdditionalAction, Some(Action::InstallDriver))
                    }
                    None => (Status::Unavailable, None),
                }
            };
            assert_eq!(
                model_availability_on(m.platform(), accel, &r),
                expected,
                "{} / {accel:?}: no runtime",
                m.name
            );
            // Even an optimistic native probe must not override unsupported OS, absent
            // hardware or an NPU model artifact that the app cannot supply.
            r.provider_registered = true;
            r.usable = true;
            let expected = if hardware
                && accel.supported_on(m.platform())
                && (!accel.needs_dedicated_artifact() || (accel == A::QnnNpu && caps.npu.present))
            {
                Status::Available
            } else {
                Status::Unavailable
            };
            assert_eq!(
                model_availability_on(m.platform(), accel, &r),
                (expected, None),
                "{} / {accel:?}: successful provider probe",
                m.name
            );
            r.probe_ok = false;
            assert_eq!(
                model_availability_on(m.platform(), accel, &r),
                (Status::Unavailable, None),
                "{}: probe failure must remain unknown/unavailable",
                m.name
            );
        }
    }
}

#[test]
fn provider_presence_registration_device_count_and_errors_are_separate() {
    for accel in ALL_ACCELERATORS {
        for present in [false, true] {
            for registered in [false, true] {
                for devices in [0, 1, 2] {
                    for error in [None, Some("simulated loader/driver failure".into())] {
                        let probe = lw_ort::AcceleratorStatus {
                            accel,
                            present,
                            registered,
                            devices,
                            error,
                        };
                        let expected = accel == A::Cpu
                            || (present && registered && devices > 0 && probe.error.is_none());
                        assert_eq!(probe.usable(), expected, "{probe:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn unavailable_models_and_failed_checks_never_become_selectable_on_any_platform() {
    for os in ["windows", "macos", "linux"] {
        for arch in ["x64", "arm64"] {
            let platform = Platform::from_names(os, arch);
            for accel in ALL_ACCELERATORS.into_iter().filter(|a| a.supported_on(platform)) {
                let mut r = ready();
                for missing in ["model", "hardware", "probe"] {
                    r.model_ready = missing != "model";
                    r.hardware_present = missing != "hardware";
                    r.probe_ok = missing != "probe";
                    assert_eq!(
                        model_availability_on(platform, accel, &r),
                        (Status::Unavailable, None),
                        "{os}/{arch}/{accel:?}: missing {missing}"
                    );
                }
                r = ready();
                r.parakeet = false;
                let expected = if accel == A::Cpu {
                    Status::Available
                } else {
                    Status::Unavailable
                };
                assert_eq!(model_availability_on(platform, accel, &r), (expected, None));
                r = ready();
                if accel.kind() == lw_core::capabilities::AcceleratorKind::Gpu {
                    r.gpu_model_ready = false;
                    assert_eq!(
                        model_availability_on(platform, accel, &r),
                        (Status::NeedAdditionalAction, Some(Action::DownloadGpuModel)),
                        "{os}/{arch}/{accel:?}"
                    );
                    r.gpu_model_ready = true;
                    r.gpu_check_failed = true;
                    assert_eq!(
                        model_availability_on(platform, accel, &r),
                        (Status::NeedAdditionalAction, Some(Action::DownloadGpuModel))
                    );
                }
            }
        }
    }
}

#[test]
fn windows_driver_store_is_emulated_on_every_test_host() {
    let root = tempfile::tempdir().unwrap();
    assert!(!detect_npu_in(&root.path().join("absent")).present);
    for (package, arch) in [
        ("qcnspmcdm_old", 73),
        ("QCNSPMCDM_new", 81),
        ("unrelated_gpu_driver", 99),
    ] {
        let dir = root.path().join(package);
        std::fs::create_dir_all(dir.join("HTP")).unwrap();
        std::fs::write(dir.join("HTP").join(format!("libQnnHtpV{arch}SkelDrv.so")), []).unwrap();
        let mut bytes = vec![0xff, 0xfe];
        for unit in "[Version]\r\nDriverVer = 07/18/2025,1.0.1.1\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(dir.join("driver.INF"), bytes).unwrap();
    }
    let npu = detect_npu_in(root.path());
    assert!(npu.present);
    assert_eq!(npu.htp_arch.unwrap().num(), 81);
    assert_eq!(npu.soc_model, Some(88));
    assert_eq!(npu.driver_version.as_deref(), Some("1.0.1.1"));
    let incomplete = tempfile::tempdir().unwrap();
    let htp = incomplete.path().join("qcnspmcdm_incomplete/HTP");
    std::fs::create_dir_all(&htp).unwrap();
    std::fs::write(htp.join("QnnHtpV81.dll"), []).unwrap();
    assert!(
        !detect_npu_in(incomplete.path()).present,
        "a helper DLL is not an NPU driver"
    );
    std::fs::write(htp.join("QnnHtpV73StubDrv.dll"), []).unwrap();
    let npu = detect_npu_in(incomplete.path());
    assert_eq!(npu.htp_arch.unwrap().num(), 73);
    assert_eq!(npu.soc_model, Some(60));
    assert_eq!(npu.driver_version, None, "missing INF must not invent a version");
}

#[test]
fn linux_sysfs_is_emulated_on_every_test_host() {
    let root = tempfile::tempdir().unwrap();
    for (node, vendor) in [
        ("card0", "0x10DE\n"),
        ("card1", "0x8086"),
        ("card10", "0x1002"),
        ("card3", "0xffff"),
        ("card0-HDMI-A-1", "0x10de"),
        ("renderD128", "0x10de"),
        ("card", "0x10de"),
    ] {
        let dir = root.path().join(node).join("device");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vendor"), vendor).unwrap();
    }
    std::fs::create_dir_all(root.path().join("card2")).unwrap(); // unreadable/missing vendor
    let devices: BTreeSet<_> = linux_gpu_descriptions(root.path()).into_iter().collect();
    assert_eq!(
        devices,
        BTreeSet::from([
            "Linux display: NVIDIA GPU (card0)".into(),
            "Linux display: Intel GPU (card1)".into(),
            "Linux display: AMD GPU (card10)".into(),
            "Linux display: Other GPU (card3)".into(),
        ])
    );
    assert!(linux_gpu_descriptions(&root.path().join("missing")).is_empty());
}

#[test]
fn driver_update_targets_use_pci_ids_without_vendor_display_names() {
    use lw_platform::drivers::display_driver_targets;
    let ids = vec![
        r"PCI\VEN_8086&DEV_A7A0&SUBSYS_12345678".into(),
        r"PCI\VEN_10DE&DEV_2704&SUBSYS_12345678".into(),
        r"pci\ven_1002&dev_744c".into(),
        r"PCI\VEN_10DE&DEV_2704&REV_A1".into(), // duplicate device
        r"ROOT\RDP".into(),
        "untrusted input".into(),
        r"PCI\VEN_ZZZZ&DEV_1234".into(),
    ];
    for accel in ["cuda", "tensor_rt"] {
        assert_eq!(
            display_driver_targets(accel, &ids).unwrap(),
            vec![r"PCI\VEN_10DE&DEV_2704"]
        );
    }
    assert_eq!(
        display_driver_targets("open_vino", &ids).unwrap(),
        vec![r"PCI\VEN_8086&DEV_A7A0"]
    );
    for accel in ["web_gpu", "direct_ml"] {
        assert_eq!(
            display_driver_targets(accel, &ids).unwrap(),
            vec![
                r"PCI\VEN_8086&DEV_A7A0",
                r"PCI\VEN_10DE&DEV_2704",
                r"PCI\VEN_1002&DEV_744C"
            ]
        );
    }
    assert!(display_driver_targets("qnn_npu", &ids).is_err());
    assert!(display_driver_targets("cuda", &ids[..1]).is_err());
    assert!(display_driver_targets("direct_ml", &[]).is_err());
}
