//! Which Vulkan devices ggml may use.
//!
//! ggml-vulkan, and the TTS, ASR and aligner on top of it, take the first GPU it lists. A GPU
//! can be listed and still fail every buffer mapping (for example a virtio-gpu whose host
//! window is too small), and whether it works can change from one desktop session to the next.
//! Before the first ggml call, lectover therefore tests every GPU in a child process
//! (`vkprobe`) and limits ggml, through `GGML_VK_VISIBLE_DEVICES`, to the GPUs that pass:
//! those on a PCI render node (physical) first, virtual ones only when no physical GPU passes.

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

/// How long one device test may take; a hang counts as failed.
#[cfg(target_os = "linux")]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a finished test child failed, from its exit status and stderr.
#[cfg(target_os = "linux")]
fn failure(status: std::process::ExitStatus, stderr: &str) -> Outcome {
    use std::os::unix::process::ExitStatusExt;
    if status.success() {
        return Outcome::Passed;
    }
    if let Some(sig) = status.signal() {
        return Outcome::Failed(format!("crashed (signal {sig})"));
    }
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let why = lines
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("error:"))
        .map(str::trim)
        .or(lines.last().copied())
        .map(|l| l.chars().take(120).collect())
        .unwrap_or_else(|| format!("exit {}", status.code().unwrap_or(-1)));
    Outcome::Failed(why)
}

/// Run one test child per device index, all at once, each with `timeout`; stderr goes to a
/// temporary file, so a noisy child cannot block. Err only when no child could start.
#[cfg(target_os = "linux")]
pub fn probe_all(
    indices: &[usize],
    spawn: &dyn Fn(usize) -> std::process::Command,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<(usize, Outcome)>> {
    use std::io::{Read, Seek};
    use std::process::Stdio;
    use std::time::Instant;

    struct Running {
        index: usize,
        child: std::process::Child,
        stderr: std::fs::File,
    }
    let started = Instant::now();
    let mut results: Vec<(usize, Outcome)> = Vec::new();
    let mut running: Vec<Running> = Vec::new();
    let mut last_error = None;
    for &index in indices {
        let spawned = tempfile::tempfile().and_then(|f| {
            let child = spawn(index)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(f.try_clone()?))
                .spawn()?;
            Ok((child, f))
        });
        match spawned {
            Ok((child, stderr)) => running.push(Running {
                index,
                child,
                stderr,
            }),
            Err(e) => {
                results.push((
                    index,
                    Outcome::Failed(format!("could not start the test: {e}")),
                ));
                last_error = Some(e);
            }
        }
    }
    if running.is_empty() {
        if let Some(e) = last_error {
            return Err(e.into());
        }
    }
    while !running.is_empty() {
        let timed_out = started.elapsed() >= timeout;
        let mut i = 0;
        while i < running.len() {
            let r = &mut running[i];
            let done = match r.child.try_wait() {
                Ok(Some(status)) => {
                    let mut text = String::new();
                    let _ = r.stderr.rewind();
                    let mut bytes = Vec::new();
                    let _ = r.stderr.read_to_end(&mut bytes);
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    Some(failure(status, &text))
                }
                Ok(None) if timed_out => {
                    let _ = r.child.kill();
                    let _ = r.child.wait();
                    Some(Outcome::Failed(format!(
                        "timed out after {} s",
                        timeout.as_secs_f64()
                    )))
                }
                Ok(None) => None,
                Err(e) => Some(Outcome::Failed(format!("lost the test: {e}"))),
            };
            if let Some(outcome) = done {
                let r = running.swap_remove(i);
                results.push((r.index, outcome));
            } else {
                i += 1;
            }
        }
        if !running.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    results.sort_by_key(|(i, _)| *i);
    Ok(results)
}

/// Test every Vulkan GPU in a child process and let ggml use only those that pass, physical
/// before virtual (see the module doc). Skipped when `GGML_VK_VISIBLE_DEVICES` is set already or
/// `device` names another backend. Call it in `main` for the commands that use the GPU, before
/// any ggml call and before other threads exist.
pub fn limit_vulkan_devices(device: Option<super::Device>) {
    #[cfg(target_os = "linux")]
    {
        if !super::Device::Vulkan.compiled()
            || std::env::var_os(VISIBLE_DEVICES_ENV).is_some()
            || device.is_some_and(|d| d != super::Device::Vulkan)
        {
            return;
        }
        // On failure ggml enumerates on its own and reports what it finds.
        let Ok(devs) = enumerate() else { return };
        let gpus: Vec<&VkDevice> = devs.iter().filter(|d| !d.cpu).collect();
        if gpus.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("vulkan: device test not possible: {e}");
                return;
            }
        };
        let spawn = |i: usize| {
            let mut c = std::process::Command::new(&exe);
            c.arg("vk-probe").env(VISIBLE_DEVICES_ENV, i.to_string());
            c
        };
        let indices: Vec<usize> = gpus.iter().map(|d| d.index).collect();
        let outcomes = match probe_all(&indices, &spawn, PROBE_TIMEOUT) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("vulkan: device test not possible: {e}");
                return;
            }
        };
        let probed: Vec<Probed> = gpus
            .iter()
            .zip(outcomes)
            .map(|(d, (_, outcome))| Probed {
                index: d.index,
                name: d.name.clone(),
                pci: d.render.is_some_and(|(a, b)| sysfs_is_pci(a, b)),
                outcome,
            })
            .collect();
        let choice = choose(&probed);
        if let Some(m) = choice.message(started.elapsed()) {
            eprintln!("{m}");
        }
        match &choice {
            Choice::Use { env, .. } => std::env::set_var(VISIBLE_DEVICES_ENV, env),
            Choice::NoGpu { .. } => std::env::set_var(VISIBLE_DEVICES_ENV, ""),
            Choice::Leave => {}
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = device;
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

    #[cfg(target_os = "linux")]
    fn sh(scripts: &'static [&'static str]) -> impl Fn(usize) -> std::process::Command {
        move |i| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(scripts[i]);
            c
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_all_passes_and_fails() {
        let spawn = sh(&[
            "exit 0",
            "echo noise >&2; echo 'error: mapMemory failed' >&2; exit 1",
        ]);
        let got = probe_all(&[0, 1], &spawn, Duration::from_secs(10)).unwrap();
        assert_eq!(
            got,
            vec![
                (0, Outcome::Passed),
                (1, Outcome::Failed("mapMemory failed".into()))
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_all_reports_a_crash() {
        let got = probe_all(&[0], &sh(&["kill -ABRT $$"]), Duration::from_secs(10)).unwrap();
        assert_eq!(got, vec![(0, Outcome::Failed("crashed (signal 6)".into()))]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_all_kills_a_hang() {
        let t = std::time::Instant::now();
        let got = probe_all(&[0], &sh(&["sleep 30"]), Duration::from_millis(300)).unwrap();
        assert_eq!(
            got,
            vec![(0, Outcome::Failed("timed out after 0.3 s".into()))]
        );
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_all_survives_a_noisy_child() {
        let t = std::time::Instant::now();
        let spawn = sh(&["head -c 2000000 /dev/zero | tr '\\0' x >&2; exit 0"]);
        let got = probe_all(&[0], &spawn, Duration::from_secs(20)).unwrap();
        assert_eq!(got, vec![(0, Outcome::Passed)]);
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn probe_all_reports_spawn_failure() {
        let spawn = |_: usize| std::process::Command::new("/nonexistent/lectover");
        assert!(probe_all(&[0, 1], &spawn, Duration::from_secs(1)).is_err());
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
