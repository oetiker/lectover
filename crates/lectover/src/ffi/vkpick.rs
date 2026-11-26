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

/// What the test of one device found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    /// Why it failed, in a few words.
    Failed(String),
}

/// One non-CPU device with its test result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probed {
    pub index: usize,
    pub name: String,
    /// On a PCI render node, i.e. a physical GPU.
    pub pci: bool,
    pub outcome: Outcome,
}

/// What to do with `GGML_VK_VISIBLE_DEVICES`.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    /// Nothing to drop: leave ggml's own choice.
    Leave,
    /// Set the variable to `env`; `skipped` names the devices left out and why.
    Use { env: String, skipped: Vec<String> },
    /// No device passed: hide every Vulkan device, so the CPU runs.
    NoGpu { skipped: Vec<String> },
}

/// The devices to use: the physical ones that passed, else the virtual ones that passed.
pub fn choose(devs: &[Probed]) -> Choice {
    let passed = |d: &&Probed| d.outcome == Outcome::Passed;
    let physical: Vec<&Probed> = devs.iter().filter(passed).filter(|d| d.pci).collect();
    let keep = if physical.is_empty() {
        devs.iter().filter(passed).collect()
    } else {
        physical
    };
    let skipped: Vec<String> = devs
        .iter()
        .filter(|d| !keep.iter().any(|k| k.index == d.index))
        .map(|d| match &d.outcome {
            Outcome::Passed => format!("{} {} (works, virtual)", d.index, d.name),
            Outcome::Failed(why) => format!("{} {} (test failed: {why})", d.index, d.name),
        })
        .collect();
    if keep.is_empty() {
        if skipped.is_empty() {
            Choice::Leave
        } else {
            Choice::NoGpu { skipped }
        }
    } else if skipped.is_empty() {
        Choice::Leave
    } else {
        let env = keep
            .iter()
            .map(|d| d.index.to_string())
            .collect::<Vec<_>>()
            .join(",");
        Choice::Use { env, skipped }
    }
}

impl Choice {
    /// The stderr line that reports the choice; `None` when nothing changes.
    pub fn message(&self, took: std::time::Duration) -> Option<String> {
        let took = took.as_secs_f64();
        match self {
            Choice::Leave => None,
            Choice::Use { env, skipped } => Some(format!(
                "vulkan: using device {env}; skipped: {}; test took {took:.1} s; set {VISIBLE_DEVICES_ENV} to choose",
                skipped.join(", ")
            )),
            Choice::NoGpu { skipped } => Some(format!(
                "vulkan: no GPU passed the test ({}); test took {took:.1} s; using the CPU; set {VISIBLE_DEVICES_ENV} to choose",
                skipped.join(", ")
            )),
        }
    }
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
        // Task 3 replaces this with a real test per device.
        let started = std::time::Instant::now();
        let probed: Vec<Probed> = devs
            .iter()
            .filter(|d| !d.cpu)
            .map(|d| Probed {
                index: d.index,
                name: d.name.clone(),
                pci: d.render.is_some_and(|(a, b)| sysfs_is_pci(a, b)),
                outcome: Outcome::Passed,
            })
            .collect();
        let choice = choose(&probed);
        if let Some(m) = choice.message(started.elapsed()) {
            eprintln!("{m}");
        }
        if let Choice::Use { env, .. } = &choice {
            std::env::set_var(VISIBLE_DEVICES_ENV, env);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn p(index: usize, name: &str, pci: bool, ok: Result<(), &str>) -> Probed {
        Probed {
            index,
            name: name.into(),
            pci,
            outcome: match ok {
                Ok(()) => Outcome::Passed,
                Err(e) => Outcome::Failed(e.into()),
            },
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn physical_pass_beats_working_virtual() {
        let devs = [
            p(0, "RADV", false, Err("mapMemory failed")),
            p(1, "Venus", false, Ok(())),
            p(2, "RADV", false, Err("x")),
            p(3, "Radeon", true, Ok(())),
        ];
        assert_eq!(
            choose(&devs),
            Choice::Use {
                env: "3".into(),
                skipped: strings(&[
                    "0 RADV (test failed: mapMemory failed)",
                    "1 Venus (works, virtual)",
                    "2 RADV (test failed: x)",
                ]),
            }
        );
    }

    #[test]
    fn virtual_is_used_when_no_physical_passes() {
        let devs = [p(0, "Venus", false, Ok(())), p(1, "Radeon", true, Err("e"))];
        assert_eq!(
            choose(&devs),
            Choice::Use {
                env: "0".into(),
                skipped: strings(&["1 Radeon (test failed: e)"]),
            }
        );
    }

    #[test]
    fn all_passing_physical_are_kept_in_order() {
        let devs = [
            p(0, "V", false, Err("e")),
            p(1, "A", true, Ok(())),
            p(2, "B", true, Ok(())),
        ];
        assert!(matches!(choose(&devs), Choice::Use { env, .. } if env == "1,2"));
    }

    #[test]
    fn nothing_to_drop_leaves_ggml_alone() {
        assert_eq!(choose(&[p(0, "Radeon", true, Ok(()))]), Choice::Leave);
        assert_eq!(choose(&[p(0, "Venus", false, Ok(()))]), Choice::Leave);
        assert_eq!(choose(&[]), Choice::Leave);
    }

    #[test]
    fn no_passing_gpu_means_cpu() {
        let devs = [p(0, "V", false, Err("a")), p(1, "R", true, Err("b"))];
        assert_eq!(
            choose(&devs),
            Choice::NoGpu {
                skipped: strings(&["0 V (test failed: a)", "1 R (test failed: b)"]),
            }
        );
    }

    #[test]
    fn message_text() {
        let took = Duration::from_millis(1400);
        let used = Choice::Use {
            env: "3".into(),
            skipped: strings(&["1 Venus (works, virtual)"]),
        };
        assert_eq!(
            used.message(took).as_deref(),
            Some(
                "vulkan: using device 3; skipped: 1 Venus (works, virtual); test took 1.4 s; \
                 set GGML_VK_VISIBLE_DEVICES to choose"
            )
        );
        let none = Choice::NoGpu {
            skipped: strings(&["0 V (test failed: a)"]),
        };
        assert_eq!(
            none.message(took).as_deref(),
            Some(
                "vulkan: no GPU passed the test (0 V (test failed: a)); test took 1.4 s; \
                 using the CPU; set GGML_VK_VISIBLE_DEVICES to choose"
            )
        );
        assert_eq!(Choice::Leave.message(took), None);
    }

    /// Prints what this machine's Vulkan loader reports:
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
    }
}
