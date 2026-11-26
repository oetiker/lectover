//! The device test that `vkpick` runs in a child process (`lectover vk-probe`, hidden), with
//! `GGML_VK_VISIBLE_DEVICES` set to one device: start ggml's Vulkan backend, allocate one
//! buffer, run one addition, check the result. A broken device may abort the process; that is
//! why it runs in a child.

use std::ffi::c_void;
use std::ptr;

use anyhow::{anyhow, bail, Result};
use lectover_sys as sys;

use super::c_str;

/// Elements per tensor: three f32 tensors of 128 MiB, one 384 MiB buffer (a virtio GPU that
/// cannot map its memory fails from 96 MiB).
const N: usize = 1 << 25;

/// Frees what the test allocated, on every path.
struct Held {
    ctx: *mut sys::ggml_context,
    backend: sys::ggml_backend_t,
    buffer: sys::ggml_backend_buffer_t,
}

impl Drop for Held {
    fn drop(&mut self) {
        // SAFETY: each handle is null or was returned by its ggml constructor and is freed once.
        unsafe {
            if !self.buffer.is_null() {
                sys::ggml_backend_buffer_free(self.buffer);
            }
            if !self.backend.is_null() {
                sys::ggml_backend_free(self.backend);
            }
            if !self.ctx.is_null() {
                sys::ggml_free(self.ctx);
            }
        }
    }
}

/// Run the test on the first Vulkan device ggml lists.
pub fn run_probe() -> Result<()> {
    // SAFETY: registry queries without preconditions; indices stay below the count.
    let dev = unsafe {
        (0..sys::ggml_backend_dev_count())
            .map(|i| sys::ggml_backend_dev_get(i))
            .find(|&d| {
                c_str(sys::ggml_backend_reg_name(
                    sys::ggml_backend_dev_backend_reg(d),
                )) == "Vulkan"
            })
    }
    .ok_or_else(|| anyhow!("no Vulkan device"))?;
    let mut held = Held {
        ctx: ptr::null_mut(),
        backend: ptr::null_mut(),
        buffer: ptr::null_mut(),
    };
    // SAFETY: `dev` comes from the registry; a null params string is allowed.
    held.backend = unsafe { sys::ggml_backend_dev_init(dev, ptr::null()) };
    if held.backend.is_null() {
        bail!("backend init failed");
    }
    let params = sys::ggml_init_params {
        mem_size: 3 * unsafe { sys::ggml_tensor_overhead() }
            + unsafe { sys::ggml_graph_overhead() },
        mem_buffer: ptr::null_mut(),
        no_alloc: true,
    };
    // SAFETY: metadata-only context (no_alloc); freed by `held`.
    held.ctx = unsafe { sys::ggml_init(params) };
    if held.ctx.is_null() {
        bail!("ggml_init failed");
    }
    let bytes = N * std::mem::size_of::<f32>();
    // SAFETY: tensors and graph live in `held.ctx`; the buffer holds their data and is freed by
    // `held`; host slices have exactly `bytes` bytes.
    unsafe {
        let a = sys::ggml_new_tensor_1d(held.ctx, sys::GGML_TYPE_F32, N as i64);
        let b = sys::ggml_new_tensor_1d(held.ctx, sys::GGML_TYPE_F32, N as i64);
        let c = sys::ggml_add(held.ctx, a, b);
        let graph = sys::ggml_new_graph(held.ctx);
        sys::ggml_build_forward_expand(graph, c);
        held.buffer = sys::ggml_backend_alloc_ctx_tensors(held.ctx, held.backend);
        if held.buffer.is_null() {
            bail!("allocating {} MiB failed", (3 * bytes) >> 20);
        }
        // One host vector for both inputs and the result: 128 MiB instead of 384 MiB per child.
        let mut host = vec![1.0f32; N];
        sys::ggml_backend_tensor_set(a, host.as_ptr().cast::<c_void>(), 0, bytes);
        host.fill(2.0);
        sys::ggml_backend_tensor_set(b, host.as_ptr().cast::<c_void>(), 0, bytes);
        if sys::ggml_backend_graph_compute(held.backend, graph) != sys::GGML_STATUS_SUCCESS {
            bail!("compute failed");
        }
        host.fill(0.0);
        sys::ggml_backend_tensor_get(c, host.as_mut_ptr().cast::<c_void>(), 0, bytes);
        if let Some((i, v)) = host.iter().enumerate().find(|(_, &v)| v != 3.0) {
            bail!("wrong result at {i}: {v}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Runs the test on this machine's first Vulkan device (Vulkan build only):
    /// `cargo test -p lectover --features vulkan --lib vkprobe -- --ignored`
    #[test]
    #[ignore = "needs a Vulkan build and device"]
    fn live() {
        super::run_probe().unwrap();
    }
}
