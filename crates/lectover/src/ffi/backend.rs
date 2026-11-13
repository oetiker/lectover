//! Compute device selection over ggml's backend registry.

use std::ffi::c_char;
use std::fmt;
use std::mem::{offset_of, size_of};

use anyhow::{anyhow, bail, Result};
use lectover_sys as sys;

use super::c_str;

/// A ggml compute backend. GPU backends exist only when the matching cargo feature is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cpu,
    Metal,
    Cuda,
    Hip,
    Vulkan,
}

impl Device {
    /// ggml's registry name for this backend (devices are named `<reg><index>`, e.g. `MTL0`).
    fn reg_name(self) -> &'static str {
        match self {
            Device::Cpu => "CPU",
            Device::Metal => "MTL",
            Device::Cuda => "CUDA",
            Device::Hip => "ROCm",
            Device::Vulkan => "Vulkan",
        }
    }

    fn from_reg_name(name: &str) -> Option<Device> {
        [
            Device::Cpu,
            Device::Metal,
            Device::Cuda,
            Device::Hip,
            Device::Vulkan,
        ]
        .into_iter()
        .find(|d| d.reg_name() == name)
    }

    /// The cargo feature that compiles this backend in (None for the CPU).
    fn feature(self) -> Option<&'static str> {
        match self {
            Device::Cpu => None,
            Device::Metal => Some("metal"),
            Device::Cuda => Some("cuda"),
            Device::Hip => Some("hip"),
            Device::Vulkan => Some("vulkan"),
        }
    }

    pub(crate) fn compiled(self) -> bool {
        match self {
            Device::Cpu => true,
            Device::Metal => cfg!(feature = "metal"),
            Device::Cuda => cfg!(feature = "cuda"),
            Device::Hip => cfg!(feature = "hip"),
            Device::Vulkan => cfg!(feature = "vulkan"),
        }
    }
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Device::Cpu => "CPU",
            Device::Metal => "Metal",
            Device::Cuda => "CUDA",
            Device::Hip => "HIP",
            Device::Vulkan => "Vulkan",
        })
    }
}

/// One entry of ggml's device registry.
struct GgmlDevice {
    reg: String,
    name: String,
    description: String,
    ty: sys::ggml_backend_dev_type,
}

impl GgmlDevice {
    fn gpu(&self) -> bool {
        self.ty == sys::GGML_BACKEND_DEVICE_TYPE_GPU
            || self.ty == sys::GGML_BACKEND_DEVICE_TYPE_IGPU
    }
}

fn ggml_devices() -> Vec<GgmlDevice> {
    // SAFETY: registry queries without preconditions; indices stay below the reported count and
    // the returned names are NUL-terminated strings owned by the (static) registry.
    unsafe {
        (0..sys::ggml_backend_dev_count())
            .map(|i| {
                let dev = sys::ggml_backend_dev_get(i);
                let reg = sys::ggml_backend_dev_backend_reg(dev);
                GgmlDevice {
                    reg: c_str(sys::ggml_backend_reg_name(reg)),
                    name: c_str(sys::ggml_backend_dev_name(dev)),
                    description: c_str(sys::ggml_backend_dev_description(dev)),
                    ty: sys::ggml_backend_dev_type(dev),
                }
            })
            .collect()
    }
}

fn first_device(d: Device) -> Option<GgmlDevice> {
    ggml_devices().into_iter().find(|g| g.reg == d.reg_name())
}

/// ggml device name for `d` (`CPU`, `MTL0`, `CUDA0`, ...), for `qt_init_params.device`.
pub(super) fn ggml_device_name(d: Device) -> Result<String> {
    first_device(d)
        .map(|g| g.name)
        .ok_or_else(|| anyhow!("no {d} device available"))
}

/// The device qwen3-asr.cpp picks on its own (it has no device parameter): the first device of
/// type GPU, else the first integrated GPU (its `gpu_device()`), else the CPU.
pub fn asr_library_device() -> Device {
    asr_device_in(&ggml_devices())
}

/// [`asr_library_device`] over a given device list.
fn asr_device_in(devices: &[GgmlDevice]) -> Device {
    [
        sys::GGML_BACKEND_DEVICE_TYPE_GPU,
        sys::GGML_BACKEND_DEVICE_TYPE_IGPU,
    ]
    .into_iter()
    .find_map(|ty| {
        devices
            .iter()
            .filter(|g| g.ty == ty)
            .find_map(|g| Device::from_reg_name(&g.reg))
    })
    .unwrap_or(Device::Cpu)
}

/// The device to run on: the requested one if it is compiled in and present, otherwise
/// (`None`) the first GPU in ggml's registry, else the CPU.
pub fn resolve_device(requested: Option<Device>) -> Result<Device> {
    match requested {
        Some(d) => {
            if !d.compiled() {
                let f = d.feature().unwrap_or_default();
                bail!("{d} requested, but this lectover was built without it: rebuild with `--features {f}`");
            }
            if first_device(d).is_none() {
                let have: Vec<String> = ggml_devices().into_iter().map(|g| g.name).collect();
                bail!(
                    "{d} requested, but ggml found no {d} device (devices: {})",
                    have.join(", ")
                );
            }
            Ok(d)
        }
        None => Ok(ggml_devices()
            .iter()
            .filter(|g| g.gpu())
            .find_map(|g| Device::from_reg_name(&g.reg))
            .unwrap_or(Device::Cpu)),
    }
}

/// One line for the log: the device in use and which GPU backends this build carries.
pub fn backend_banner(d: Device) -> String {
    let which = match first_device(d) {
        Some(g) => format!("{d} ({}: {})", g.name, g.description),
        None => format!("{d} (not available)"),
    };
    let flags: Vec<String> = [Device::Metal, Device::Cuda, Device::Hip, Device::Vulkan]
        .into_iter()
        .map(|d| {
            format!(
                "{}={}",
                d.feature().unwrap_or_default(),
                if d.compiled() { "on" } else { "off" }
            )
        })
        .collect();
    format!("backend: {which}; built with {}", flags.join(" "))
}

/// `GGML_MAX_NAME` the bindings were generated with (lectover-sys/build.rs).
const GGML_MAX_NAME: usize = 128;
/// `sizeof(struct ggml_object)` in ggml.c on 64-bit targets (3 × 8-byte fields, enum, padding).
const GGML_OBJECT_SIZE: usize = 32;

/// Check that the bindgen layout of `ggml_tensor` matches the compiled C side. `GGML_MAX_NAME`
/// sizes `ggml_tensor::name`; if build.rs and the CMake build ever disagree, every field after
/// `name` would be read at the wrong offset.
pub fn check_ggml_layout() -> Result<()> {
    let rust_name_len = sys::ggml_tensor::default().name.len();
    if rust_name_len != GGML_MAX_NAME {
        bail!("bindings: ggml_tensor.name has {rust_name_len} bytes, expected {GGML_MAX_NAME}");
    }
    // ggml_tensor_overhead() = GGML_OBJECT_SIZE + sizeof(struct ggml_tensor) as compiled in C.
    // SAFETY: no preconditions.
    let overhead = unsafe { sys::ggml_tensor_overhead() };
    if size_of::<usize>() == 8 && overhead != GGML_OBJECT_SIZE + size_of::<sys::ggml_tensor>() {
        bail!(
            "C sizeof(ggml_tensor) = {} but bindings say {}",
            overhead - GGML_OBJECT_SIZE,
            size_of::<sys::ggml_tensor>()
        );
    }
    let params = sys::ggml_init_params {
        mem_size: 4 * overhead,
        mem_buffer: std::ptr::null_mut(),
        no_alloc: true,
    };
    // SAFETY: a tiny no_alloc context owns one metadata-only tensor; the name is a valid
    // NUL-terminated string that ggml_set_name copies (truncating to its own GGML_MAX_NAME - 1);
    // the context is freed before returning and no pointer into it escapes.
    let (name_offset, c_name_len) = unsafe {
        let ctx = sys::ggml_init(params);
        if ctx.is_null() {
            bail!("ggml_init failed");
        }
        let t = sys::ggml_new_tensor_1d(ctx, sys::GGML_TYPE_F32, 1);
        let long = [b'x' as c_char; 2 * GGML_MAX_NAME]
            .into_iter()
            .chain([0])
            .collect::<Vec<c_char>>();
        sys::ggml_set_name(t, long.as_ptr());
        let name = sys::ggml_get_name(t);
        let r = (name as usize - t as usize, c_str(name).len());
        sys::ggml_free(ctx);
        r
    };
    if name_offset != offset_of!(sys::ggml_tensor, name) {
        bail!(
            "C offset of ggml_tensor.name = {name_offset}, bindings say {}",
            offset_of!(sys::ggml_tensor, name)
        );
    }
    if c_name_len != GGML_MAX_NAME - 1 {
        bail!(
            "C side was compiled with GGML_MAX_NAME = {}, bindings with {GGML_MAX_NAME}",
            c_name_len + 1
        );
    }
    Ok(())
}

/// Whether a ggml log line of `level` is printed. WARN and ERROR always; everything else
/// (DEBUG, INFO, CONT, NONE) only when `verbose`.
pub(super) fn ggml_log_passes(level: sys::ggml_log_level, verbose: bool) -> bool {
    verbose || level == sys::GGML_LOG_LEVEL_WARN || level == sys::GGML_LOG_LEVEL_ERROR
}

unsafe extern "C" fn ggml_log_cb(
    level: sys::ggml_log_level,
    text: *const c_char,
    user: *mut std::ffi::c_void,
) {
    use std::io::Write;
    if text.is_null() || !ggml_log_passes(level, !user.is_null()) {
        return;
    }
    // SAFETY: ggml passes a NUL-terminated string valid for this call.
    let bytes = unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes();
    let _ = std::io::stderr().write_all(bytes); // never unwind into C
}

/// Install lectover's ggml log filter (once per process): WARN and ERROR reach stderr,
/// the rest is dropped unless `LECTOVER_GGML_LOG` is `debug` or `all`. The TTS may replace
/// the callback later; that is fine.
pub fn init_ggml_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let verbose = matches!(
            std::env::var("LECTOVER_GGML_LOG").as_deref(),
            Ok("debug") | Ok("all")
        );
        let user = if verbose {
            std::ptr::dangling_mut::<std::ffi::c_void>()
        } else {
            std::ptr::null_mut()
        };
        // SAFETY: the callback is a plain fn; `user` is only a flag, never dereferenced.
        unsafe { sys::ggml_log_set(Some(ggml_log_cb), user) };
    });
}

/// ggml's CUDA/HIP backend allocates managed memory whenever this variable exists (any value,
/// even `0`). On Strix Halo (HIP) that makes the TTS code predictor compute garbage:
/// takes never end and the ASR reports "transcribe: empty audio" (test-backend-ops passes either way).
pub(crate) const UNIFIED_MEMORY_ENV: &str = "GGML_CUDA_ENABLE_UNIFIED_MEMORY";

/// The startup warning for `GGML_CUDA_ENABLE_UNIFIED_MEMORY`, if one is due: only when the
/// variable is set and the binary has a backend that reads it (CUDA or HIP).
pub(crate) fn unified_memory_warning(env_set: bool, cuda_or_hip: bool) -> Option<String> {
    (env_set && cuda_or_hip).then(|| {
        format!(
            "warning: {UNIFIED_MEMORY_ENV} is set; ggml then uses managed memory, which breaks the TTS \
             on HIP (takes never end, \"transcribe: empty audio\"). Unset it: lectover does not need it."
        )
    })
}

/// Print the `GGML_CUDA_ENABLE_UNIFIED_MEMORY` warning to stderr when it applies.
pub fn warn_unified_memory() {
    let env_set = std::env::var_os(UNIFIED_MEMORY_ENV).is_some();
    if let Some(w) =
        unified_memory_warning(env_set, Device::Cuda.compiled() || Device::Hip.compiled())
    {
        eprintln!("{w}");
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn log_filter_levels() {
        assert!(ggml_log_passes(sys::GGML_LOG_LEVEL_ERROR, false));
        assert!(ggml_log_passes(sys::GGML_LOG_LEVEL_WARN, false));
        for l in [
            sys::GGML_LOG_LEVEL_DEBUG,
            sys::GGML_LOG_LEVEL_INFO,
            sys::GGML_LOG_LEVEL_CONT,
            sys::GGML_LOG_LEVEL_NONE,
        ] {
            assert!(!ggml_log_passes(l, false));
            assert!(ggml_log_passes(l, true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(reg: &str, ty: sys::ggml_backend_dev_type) -> GgmlDevice {
        GgmlDevice {
            reg: reg.into(),
            name: format!("{reg}0"),
            description: String::new(),
            ty,
        }
    }

    #[test]
    fn asr_device_follows_the_library_gpu_then_igpu_rule() {
        use sys::{
            GGML_BACKEND_DEVICE_TYPE_CPU as CPU, GGML_BACKEND_DEVICE_TYPE_GPU as GPU,
            GGML_BACKEND_DEVICE_TYPE_IGPU as IGPU,
        };
        // an APU (Strix Halo under HIP) is an IGPU: the ASR runs there, not on the CPU
        assert_eq!(
            asr_device_in(&[dev("CPU", CPU), dev("ROCm", IGPU)]),
            Device::Hip
        );
        // a discrete GPU wins over an integrated one, wherever it is listed
        assert_eq!(
            asr_device_in(&[dev("Vulkan", IGPU), dev("CUDA", GPU)]),
            Device::Cuda
        );
        assert_eq!(asr_device_in(&[dev("MTL", GPU)]), Device::Metal);
        assert_eq!(asr_device_in(&[dev("CPU", CPU)]), Device::Cpu);
    }

    #[test]
    fn unified_memory_warning_only_when_set_on_cuda_or_hip() {
        let w = unified_memory_warning(true, true).expect("set on HIP/CUDA: warn");
        assert!(w.contains("GGML_CUDA_ENABLE_UNIFIED_MEMORY"), "{w}");
        assert_eq!(unified_memory_warning(false, true), None);
        assert_eq!(
            unified_memory_warning(true, false),
            None,
            "Metal/Vulkan/CPU builds never read it"
        );
    }
}
