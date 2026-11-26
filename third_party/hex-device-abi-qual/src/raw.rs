//! Strictly isolated POSIX `dlopen`/`dlsym`/`dlclose` + C ABI vtable boundary.
//! All `unsafe` operations for the qualification loader live exclusively in this
//! private module; callers interact only through safe validated wrappers.
#![allow(unsafe_code)]

use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, AbiProcessContext,
    AbiStatus, PACKAGE_ABI_VERSION_V1, validate_pcm_output,
};
use std::ffi::{CString, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

type DescribeFn = unsafe extern "C" fn(*mut AbiDescriptor) -> i32;
type ParamDescFn = unsafe extern "C" fn(u32, *mut AbiParameterDescriptor) -> i32;
type CreateFn = unsafe extern "C" fn(u32, *mut *mut c_void) -> i32;
type DestroyFn = unsafe extern "C" fn(*mut c_void);
type SetStateFn = unsafe extern "C" fn(*mut c_void, *const u8, u32) -> i32;
type GetStateFn = unsafe extern "C" fn(*const c_void, *mut u8, u32, *mut u32) -> i32;
type ProcessFn = unsafe extern "C" fn(
    *mut c_void,
    *const AbiProcessContext,
    *const f32,
    *mut f32,
    *const AbiParamEvent,
    *const AbiNoteEvent,
) -> i32;
type EntryFn = unsafe extern "C" fn(u32, *mut AbiVtableV1) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct AbiVtableV1 {
    struct_size: u32,
    abi_version: u32,
    describe: Option<DescribeFn>,
    parameter_descriptor: Option<ParamDescFn>,
    create: Option<CreateFn>,
    destroy: Option<DestroyFn>,
    set_state: Option<SetStateFn>,
    get_state: Option<GetStateFn>,
    process: Option<ProcessFn>,
}

struct LibraryHandle {
    raw: *mut c_void,
    vtable: AbiVtableV1,
    descriptor: AbiDescriptor,
}

// SAFETY: `LibraryHandle` holds a `dlopen` handle and immutable function pointers
// negotiated once at load time. Instances created from it retain an `Arc` to the
// handle so `dlclose` only runs after every instance has been destroyed.
unsafe impl Send for LibraryHandle {}
unsafe impl Sync for LibraryHandle {}

impl Drop for LibraryHandle {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `self.raw` was returned by `dlopen` and all instances
            // holding an `Arc<LibraryHandle>` have already been destroyed.
            unsafe {
                libc::dlclose(self.raw);
            }
        }
    }
}

pub(crate) struct RawModule {
    handle: Arc<LibraryHandle>,
}

pub(crate) struct RawInstance {
    handle: Arc<LibraryHandle>,
    ptr: *mut c_void,
    sample_rate: u32,
}

// SAFETY: Instances follow single-thread-at-a-time affinity enforced by `&mut self`.
unsafe impl Send for RawInstance {}

impl Drop for RawInstance {
    fn drop(&mut self) {
        if !self.ptr.is_null()
            && let Some(destroy) = self.handle.vtable.destroy
        {
            // SAFETY: `self.ptr` was created by `vtable.create` and is destroyed
            // exactly once while `self.handle` keeps the library mapped.
            unsafe {
                destroy(self.ptr);
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}

impl RawModule {
    pub(crate) fn load(path: &Path) -> Result<Self, AbiStatus> {
        let c_path =
            CString::new(path.as_os_str().as_bytes()).map_err(|_| AbiStatus::InvalidArg)?;
        // SAFETY: `c_path` is a valid NUL-terminated path string; `RTLD_LOCAL | RTLD_NOW`
        // keeps symbols isolated and resolves them eagerly on control.
        let raw = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_LOCAL | libc::RTLD_NOW) };
        if raw.is_null() {
            return Err(AbiStatus::InternalFault);
        }

        let symbol = CString::new("hex_device_abi_v1_entry").map_err(|_| AbiStatus::InvalidArg)?;
        // SAFETY: `raw` is a valid `dlopen` handle and `symbol` is NUL-terminated.
        let sym = unsafe { libc::dlsym(raw, symbol.as_ptr()) };
        if sym.is_null() {
            // SAFETY: Close handle on failure.
            unsafe {
                libc::dlclose(raw);
            }
            return Err(AbiStatus::UnsupportedAbi);
        }

        // SAFETY: The symbol name `hex_device_abi_v1_entry` is contractually an
        // `EntryFn` matching `include/hex_device_abi_v1.h`.
        let entry: EntryFn = unsafe { std::mem::transmute(sym) };
        let mut vtable = AbiVtableV1 {
            struct_size: std::mem::size_of::<AbiVtableV1>() as u32,
            abi_version: 0,
            describe: None,
            parameter_descriptor: None,
            create: None,
            destroy: None,
            set_state: None,
            get_state: None,
            process: None,
        };
        // SAFETY: `vtable` is initialized with its exact `struct_size`.
        let status = AbiStatus::from_i32(unsafe { entry(PACKAGE_ABI_VERSION_V1, &mut vtable) });
        if status != AbiStatus::Ok
            || vtable.abi_version != PACKAGE_ABI_VERSION_V1
            || vtable.describe.is_none()
            || vtable.parameter_descriptor.is_none()
            || vtable.create.is_none()
            || vtable.destroy.is_none()
            || vtable.set_state.is_none()
            || vtable.get_state.is_none()
            || vtable.process.is_none()
        {
            // SAFETY: Close handle on failed negotiation.
            unsafe {
                libc::dlclose(raw);
            }
            return Err(if status == AbiStatus::Ok {
                AbiStatus::UnsupportedAbi
            } else {
                status
            });
        }

        let mut descriptor = AbiDescriptor {
            struct_size: std::mem::size_of::<AbiDescriptor>() as u32,
            magic: 0,
            abi_version: 0,
            device_kind: 0,
            state_version: 0,
            ui_version: 0,
            package_major: 0,
            package_minor: 0,
            package_patch: 0,
            reserved0: 0,
            capabilities: 0,
            reported_latency_frames: 0,
            tail_frames: 0,
            parameter_count: 0,
            max_state_bytes: 0,
            type_id_len: 0,
            reserved1: 0,
            type_id: [0u8; 128],
        };
        // SAFETY: `vtable.describe` was verified non-null above.
        let desc_status = AbiStatus::from_i32(unsafe {
            (vtable.describe.expect("checked non-null"))(&mut descriptor)
        });
        if desc_status != AbiStatus::Ok {
            // SAFETY: Close handle on describe failure.
            unsafe {
                libc::dlclose(raw);
            }
            return Err(desc_status);
        }
        if let Err(err) = descriptor.validate() {
            // SAFETY: Close handle on malformed descriptor.
            unsafe {
                libc::dlclose(raw);
            }
            return Err(err);
        }

        Ok(Self {
            handle: Arc::new(LibraryHandle {
                raw,
                vtable,
                descriptor,
            }),
        })
    }

    pub(crate) fn descriptor(&self) -> AbiDescriptor {
        self.handle.descriptor
    }

    pub(crate) fn parameter_descriptor(
        &self,
        index: u32,
    ) -> Result<AbiParameterDescriptor, AbiStatus> {
        if index >= self.handle.descriptor.parameter_count {
            return Err(AbiStatus::InvalidArg);
        }
        let mut out = AbiParameterDescriptor {
            struct_size: std::mem::size_of::<AbiParameterDescriptor>() as u32,
            id: 0,
            min: 0.0,
            max: 0.0,
            default_value: 0.0,
            flags: 0,
            name_len: 0,
            reserved: 0,
            name: [0u8; 64],
        };
        let f = self
            .handle
            .vtable
            .parameter_descriptor
            .ok_or(AbiStatus::UnsupportedAbi)?;
        // SAFETY: `out` is a valid mutable descriptor buffer.
        let status = AbiStatus::from_i32(unsafe { f(index, &mut out) });
        if status != AbiStatus::Ok {
            return Err(status);
        }
        out.validate()?;
        Ok(out)
    }

    pub(crate) fn create_instance(&self, sample_rate: u32) -> Result<RawInstance, AbiStatus> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err(AbiStatus::InvalidRate);
        }
        let create = self.handle.vtable.create.ok_or(AbiStatus::UnsupportedAbi)?;
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: `ptr` is valid out-parameter storage.
        let status = AbiStatus::from_i32(unsafe { create(sample_rate, &mut ptr) });
        if status != AbiStatus::Ok || ptr.is_null() {
            return Err(if status == AbiStatus::Ok {
                AbiStatus::AllocationFailed
            } else {
                status
            });
        }
        Ok(RawInstance {
            handle: Arc::clone(&self.handle),
            ptr,
            sample_rate,
        })
    }
}

impl RawInstance {
    pub(crate) fn set_state(&mut self, bytes: &[u8]) -> Result<(), AbiStatus> {
        let f = self
            .handle
            .vtable
            .set_state
            .ok_or(AbiStatus::UnsupportedAbi)?;
        let len = u32::try_from(bytes.len()).map_err(|_| AbiStatus::InvalidState)?;
        // SAFETY: `self.ptr` is live and `bytes` stays borrowed for the call.
        let status = AbiStatus::from_i32(unsafe { f(self.ptr, bytes.as_ptr(), len) });
        if status == AbiStatus::Ok {
            Ok(())
        } else {
            Err(status)
        }
    }

    pub(crate) fn get_state(&self, out: &mut [u8]) -> Result<usize, AbiStatus> {
        let f = self
            .handle
            .vtable
            .get_state
            .ok_or(AbiStatus::UnsupportedAbi)?;
        let cap = u32::try_from(out.len()).map_err(|_| AbiStatus::InvalidArg)?;
        let mut written = 0u32;
        // SAFETY: `self.ptr` is live and `out` stays mutably borrowed for the call.
        let status =
            AbiStatus::from_i32(unsafe { f(self.ptr, out.as_mut_ptr(), cap, &mut written) });
        if status != AbiStatus::Ok {
            return Err(status);
        }
        let len = written as usize;
        if len > out.len() {
            return Err(AbiStatus::InvalidState);
        }
        Ok(len)
    }

    pub(crate) fn process(
        &mut self,
        input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
        note_events: &[AbiNoteEvent],
    ) -> Result<(), AbiStatus> {
        if input.len() != output.len() {
            return Err(AbiStatus::InvalidFrames);
        }
        let ctx = AbiProcessContext::new(
            self.sample_rate,
            u32::try_from(output.len()).map_err(|_| AbiStatus::InvalidFrames)?,
            u32::try_from(param_events.len()).map_err(|_| AbiStatus::InvalidArg)?,
            u32::try_from(note_events.len()).map_err(|_| AbiStatus::InvalidArg)?,
        );
        ctx.validate()?;
        let f = self
            .handle
            .vtable
            .process
            .ok_or(AbiStatus::UnsupportedAbi)?;
        let p_ptr = if param_events.is_empty() {
            std::ptr::null()
        } else {
            param_events.as_ptr()
        };
        let n_ptr = if note_events.is_empty() {
            std::ptr::null()
        } else {
            note_events.as_ptr()
        };
        // SAFETY: `input` and `output` are contiguous `[[f32; 2]]` slices of length
        // `ctx.frames`, i.e., `2 * frames` interleaved `f32` samples. No pointer
        // escapes past the call.
        let status = AbiStatus::from_i32(unsafe {
            f(
                self.ptr,
                &ctx,
                input.as_ptr().cast::<f32>(),
                output.as_mut_ptr().cast::<f32>(),
                p_ptr,
                n_ptr,
            )
        });
        if status != AbiStatus::Ok {
            output.fill([0.0; 2]);
            return Err(status);
        }
        if let Err(err) = validate_pcm_output(output) {
            output.fill([0.0; 2]);
            return Err(err);
        }
        Ok(())
    }
}
