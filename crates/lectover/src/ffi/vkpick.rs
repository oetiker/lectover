//! Which Vulkan devices ggml may use.
//!
//! ggml-vulkan, and the TTS, ASR and aligner on top of it, take the first GPU it lists. A
//! virtual GPU (for example a virtio-gpu with the Venus driver) can come first and fail every
//! buffer mapping. Before the first ggml call, lectover therefore limits ggml to the GPUs whose
//! DRM render node belongs to a PCI device, through `GGML_VK_VISIBLE_DEVICES`.

/// One physical device as Vulkan enumerates it; `index` is its place in that order, which is
/// what `GGML_VK_VISIBLE_DEVICES` counts.
#[derive(Debug, Clone)]
pub struct VkDevice {
    pub index: usize,
    pub name: String,
    pub cpu: bool,
    /// DRM render node (major, minor), when the driver reports one.
    pub render: Option<(i64, i64)>,
}

/// The `GGML_VK_VISIBLE_DEVICES` value to set, and the devices it leaves out.
#[derive(Debug, PartialEq, Eq)]
pub struct Pick {
    pub env: String,
    pub skipped: Vec<String>,
}

impl Pick {
    /// The stderr line that reports the choice.
    pub fn message(&self) -> String {
        format!(
            "vulkan: using device {} only; skipping the devices not on a PCI render node: {}; set {VISIBLE_DEVICES_ENV} to choose",
            self.env,
            self.skipped.join(", ")
        )
    }
}

/// The GPUs on a PCI render node, when there are some and other GPUs besides them; `None`
/// leaves ggml's own choice.
pub fn visible_devices(devs: &[VkDevice], is_pci: impl Fn(i64, i64) -> bool) -> Option<Pick> {
    let (good, bad): (Vec<&VkDevice>, Vec<&VkDevice>) = devs
        .iter()
        .filter(|d| !d.cpu)
        .partition(|d| d.render.is_some_and(|(major, minor)| is_pci(major, minor)));
    if good.is_empty() || bad.is_empty() {
        return None;
    }
    let env = good
        .iter()
        .map(|d| d.index.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let skipped = bad
        .iter()
        .map(|d| format!("{} {}", d.index, d.name))
        .collect();
    Some(Pick { env, skipped })
}

/// ggml-vulkan's list of device indices (into Vulkan's enumeration order).
pub const VISIBLE_DEVICES_ENV: &str = "GGML_VK_VISIBLE_DEVICES";

/// Whether the character device `major:minor` belongs to a PCI device, per sysfs.
#[cfg(target_os = "linux")]
fn sysfs_is_pci(major: i64, minor: i64) -> bool {
    std::fs::canonicalize(format!("/sys/dev/char/{major}:{minor}/device/subsystem"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n == "pci"))
        .unwrap_or(false)
}

/// Vulkan's physical devices, in its enumeration order, from a throwaway instance.
#[cfg(target_os = "linux")]
fn enumerate() -> anyhow::Result<Vec<VkDevice>> {
    use ash::vk;
    // SAFETY: loads the system Vulkan loader; nothing else runs Vulkan this early.
    let entry = unsafe { ash::Entry::load() }?;
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let info = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: valid create info; the instance is destroyed below and no handle escapes.
    let instance = unsafe { entry.create_instance(&info, None) }?;
    let devices = (|| -> anyhow::Result<Vec<VkDevice>> {
        // SAFETY: queries on a live instance and the physical devices it returned.
        let physical = unsafe { instance.enumerate_physical_devices() }?;
        physical
            .iter()
            .enumerate()
            .map(|(index, &pd)| {
                let exts = unsafe { instance.enumerate_device_extension_properties(pd) }?;
                let has_drm = exts.iter().any(|e| {
                    e.extension_name_as_c_str() == Ok(ash::ext::physical_device_drm::NAME)
                });
                let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
                let (name, cpu) = {
                    let mut p2 = vk::PhysicalDeviceProperties2::default();
                    if has_drm {
                        p2 = p2.push_next(&mut drm);
                    }
                    unsafe { instance.get_physical_device_properties2(pd, &mut p2) };
                    let p = p2.properties;
                    (
                        p.device_name_as_c_str()
                            .map(|c| c.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        p.device_type == vk::PhysicalDeviceType::CPU,
                    )
                };
                let render = (has_drm && drm.has_render == vk::TRUE)
                    .then_some((drm.render_major, drm.render_minor));
                Ok(VkDevice {
                    index,
                    name,
                    cpu,
                    render,
                })
            })
            .collect()
    })();
    // SAFETY: created above; nothing derived from it is used after this.
    unsafe { instance.destroy_instance(None) };
    devices
}

/// Limit ggml to the GPUs on a PCI render node when Vulkan also lists others (see the module
/// doc), unless `GGML_VK_VISIBLE_DEVICES` is set already. Call it in `main` for the commands
/// that use the GPU, before any ggml call and before other threads exist.
pub fn limit_vulkan_devices() {
    #[cfg(target_os = "linux")]
    {
        if !super::Device::Vulkan.compiled() || std::env::var_os(VISIBLE_DEVICES_ENV).is_some() {
            return;
        }
        // On failure ggml enumerates on its own and reports what it finds.
        let Ok(devs) = enumerate() else { return };
        if let Some(p) = visible_devices(&devs, sysfs_is_pci) {
            eprintln!("{}", p.message());
            std::env::set_var(VISIBLE_DEVICES_ENV, &p.env);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(index: usize, name: &str, cpu: bool, render: Option<(i64, i64)>) -> VkDevice {
        VkDevice {
            index,
            name: name.into(),
            cpu,
            render,
        }
    }

    /// ol-01 on 2026-11-26: Venus and RADV on the virtio node (minor 129), the physical
    /// Radeon (minor 128), llvmpipe.
    fn ol01() -> Vec<VkDevice> {
        vec![
            dev(
                0,
                "Virtio-GPU Venus (Radeon 8060S)",
                false,
                Some((226, 129)),
            ),
            dev(1, "Radeon 8060S (RADV)", false, Some((226, 129))),
            dev(2, "Radeon 8060S (RADV)", false, Some((226, 128))),
            dev(3, "llvmpipe", true, None),
        ]
    }

    fn pci_128(_: i64, minor: i64) -> bool {
        minor == 128
    }

    #[test]
    fn keeps_only_gpus_on_a_pci_render_node() {
        let p = visible_devices(&ol01(), pci_128).unwrap();
        assert_eq!(p.env, "2");
        assert_eq!(
            p.skipped,
            vec!["0 Virtio-GPU Venus (Radeon 8060S)", "1 Radeon 8060S (RADV)"]
        );
    }

    #[test]
    fn message_gives_the_reason_for_every_skipped_device() {
        let p = visible_devices(&ol01(), pci_128).unwrap();
        assert_eq!(
            p.message(),
            "vulkan: using device 2 only; skipping the devices not on a PCI render node: \
             0 Virtio-GPU Venus (Radeon 8060S), 1 Radeon 8060S (RADV); \
             set GGML_VK_VISIBLE_DEVICES to choose"
        );
    }

    #[test]
    fn device_without_a_render_node_is_skipped() {
        let devs = vec![
            dev(0, "Venus", false, None),
            dev(1, "Radeon", false, Some((226, 128))),
        ];
        assert_eq!(visible_devices(&devs, pci_128).unwrap().env, "1");
    }

    #[test]
    fn several_pci_gpus_are_all_kept_in_order() {
        let devs = vec![
            dev(0, "Venus", false, Some((226, 129))),
            dev(1, "A", false, Some((226, 128))),
            dev(2, "B", false, Some((226, 130))),
        ];
        let p = visible_devices(&devs, |_, m| m != 129).unwrap();
        assert_eq!(p.env, "1,2");
    }

    #[test]
    fn leaves_ggml_alone_when_nothing_is_to_skip() {
        let devs = vec![
            dev(0, "Radeon", false, Some((226, 128))),
            dev(1, "llvmpipe", true, None),
        ];
        assert!(visible_devices(&devs, pci_128).is_none());
    }

    /// Prints what this machine's Vulkan loader reports and what lectover would pick:
    /// `cargo test -p lectover --lib vkpick::tests::live -- --ignored --nocapture`
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "needs a Vulkan loader; prints this machine's devices"]
    fn live() {
        let devs = enumerate().expect("Vulkan enumeration");
        for d in &devs {
            let pci = d.render.is_some_and(|(a, b)| sysfs_is_pci(a, b));
            println!(
                "{} {:?} cpu={} render={:?} pci={pci}",
                d.index, d.name, d.cpu, d.render
            );
        }
        println!("pick: {:?}", visible_devices(&devs, sysfs_is_pci));
    }

    #[test]
    fn leaves_ggml_alone_when_no_pci_gpu_exists() {
        // A VM with only a virtual GPU: better that one than none.
        let devs = vec![dev(0, "Venus", false, Some((226, 129)))];
        assert!(visible_devices(&devs, pci_128).is_none());
        assert!(visible_devices(&[], pci_128).is_none());
    }
}
